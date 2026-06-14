use catalogy_catalog::{Catalog, CatalogRecord};
use catalogy_core::{CatalogyError, Job, JobStage, Result};
use catalogy_queue::StateDb;
use std::path::Path;

#[cfg(test)]
use crate::session::l2_normalize;
use crate::session::{dedup_frames, mean_pool, EmbedSession};

/// Run the embed worker loop: claim embed jobs, run inference, write to catalog.
/// Returns the number of jobs processed.
///
/// Images are embedded directly into one `image` catalog row. Videos read their
/// extracted frames from the metadata sidecar in `frames_meta_dir`, embed each
/// frame, deduplicate near-identical frames (cosine similarity > `dedup_threshold`),
/// and write one `video_frame` row per kept frame plus one mean-pooled `video`
/// row (LLD §3.3 / implementation plan task 4.6).
#[allow(clippy::too_many_arguments)]
pub fn run_embed_worker(
    db: &StateDb,
    session: &EmbedSession,
    catalog: &Catalog,
    model_id: &str,
    model_version: &str,
    worker_id: &str,
    frames_meta_dir: &Path,
    dedup_threshold: f32,
) -> Result<u64> {
    let mut count = 0u64;

    while let Some(job) = db.claim(JobStage::Embed, worker_id)? {
        // Check if file exists
        if !job.file_path.exists() {
            db.skip(job.id)?;
            continue;
        }

        let file_ext = job
            .file_path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();

        let result = if determine_media_type(&file_ext) == "video" {
            embed_video(
                session,
                catalog,
                &job,
                &file_ext,
                model_id,
                model_version,
                frames_meta_dir,
                dedup_threshold,
            )
        } else {
            embed_image_file(session, catalog, &job, &file_ext, model_id, model_version)
        };

        match result {
            Ok(()) => {
                db.complete(job.id)?;
                count += 1;
            }
            Err(e) => {
                db.fail(job.id, &e.to_string())?;
            }
        }
    }

    Ok(count)
}

/// Embed a single image file into one `image` catalog row.
fn embed_image_file(
    session: &EmbedSession,
    catalog: &Catalog,
    job: &Job,
    file_ext: &str,
    model_id: &str,
    model_version: &str,
) -> Result<()> {
    let embedding = session.embed_image(&job.file_path)?;
    let now = chrono::Utc::now().to_rfc3339();
    let record = build_record(
        job,
        file_ext,
        model_id,
        model_version,
        "image",
        embedding,
        None,
        None,
        None,
        &now,
    );
    catalog.upsert(&record)
}

/// Embed all extracted frames of a video, dedup in embedding space, and write
/// one `video_frame` row per kept frame plus one mean-pooled `video` row.
#[allow(clippy::too_many_arguments)]
fn embed_video(
    session: &EmbedSession,
    catalog: &Catalog,
    job: &Job,
    file_ext: &str,
    model_id: &str,
    model_version: &str,
    frames_meta_dir: &Path,
    dedup_threshold: f32,
) -> Result<()> {
    // Locate the frames extracted by the upstream extract_frames stage.
    let frames = catalogy_extract::read_frame_metadata(frames_meta_dir, &job.file_hash.0)?;
    let frames: Vec<_> = frames.into_iter().filter(|f| f.path.exists()).collect();
    if frames.is_empty() {
        return Err(CatalogyError::Embedding(format!(
            "no extracted frames found for video {} — run the frames stage first",
            job.file_path.display()
        )));
    }

    // Embed frames one at a time. The exported CLIP visual model has a fixed
    // batch dimension of 1, so batched inference (embed_images) is not usable
    // here. Frames are in frame_index (timestamp) order, which dedup relies on.
    let mut embeddings: Vec<Vec<f32>> = Vec::with_capacity(frames.len());
    for frame in &frames {
        embeddings.push(session.embed_image(&frame.path)?);
    }

    // Drop frames that are near-identical to the previous kept frame.
    let kept = dedup_frames(&embeddings, dedup_threshold);

    let now = chrono::Utc::now().to_rfc3339();
    let video_path = job.file_path.to_string_lossy().to_string();
    let mut records = Vec::with_capacity(kept.len() + 1);
    let mut kept_embeddings = Vec::with_capacity(kept.len());

    // One row per kept frame ("find the moment").
    for &i in &kept {
        let frame = &frames[i];
        kept_embeddings.push(embeddings[i].clone());
        records.push(build_record(
            job,
            file_ext,
            model_id,
            model_version,
            "video_frame",
            embeddings[i].clone(),
            Some(video_path.clone()),
            Some(frame.frame_index as i32),
            Some(frame.timestamp_ms as i64),
            &now,
        ));
    }

    // One aggregated row for the video itself ("find the video").
    let video_embedding = mean_pool(&kept_embeddings);
    records.push(build_record(
        job,
        file_ext,
        model_id,
        model_version,
        "video",
        video_embedding,
        None,
        None,
        None,
        &now,
    ));

    catalog.batch_upsert(&records)
}

/// Build a `CatalogRecord` for a job, filling the fields shared across image,
/// video, and video_frame rows. Metadata columns (dimensions, EXIF, codec) are
/// populated by the metadata stage, not here.
#[allow(clippy::too_many_arguments)]
fn build_record(
    job: &Job,
    file_ext: &str,
    model_id: &str,
    model_version: &str,
    media_type: &str,
    embedding: Vec<f32>,
    source_video_path: Option<String>,
    frame_index: Option<i32>,
    frame_timestamp_ms: Option<i64>,
    now: &str,
) -> CatalogRecord {
    let file_name = job
        .file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let file_size = std::fs::metadata(&job.file_path)
        .map(|m| m.len() as i64)
        .unwrap_or(0);

    CatalogRecord {
        id: uuid::Uuid::now_v7().to_string(),
        file_hash: job.file_hash.0.clone(),
        file_path: job.file_path.to_string_lossy().to_string(),
        file_name,
        file_size,
        file_ext: file_ext.to_string(),
        media_type: media_type.to_string(),
        embedding,
        model_id: model_id.to_string(),
        model_version: model_version.to_string(),
        width: None,
        height: None,
        duration_ms: None,
        fps: None,
        codec: None,
        bitrate_kbps: None,
        exif_camera_make: None,
        exif_camera_model: None,
        exif_date_taken: None,
        exif_gps_lat: None,
        exif_gps_lon: None,
        exif_focal_length_mm: None,
        exif_iso: None,
        exif_orientation: None,
        source_video_path,
        frame_index,
        frame_timestamp_ms,
        file_created: None,
        file_modified: None,
        indexed_at: now.to_string(),
        updated_at: now.to_string(),
        tombstone: false,
    }
}

/// After all frames for a video are embedded, deduplicate and aggregate.
/// Returns (kept_frame_indices, video_level_embedding).
pub fn aggregate_video_frames(
    frame_embeddings: &[Vec<f32>],
    dedup_threshold: f32,
) -> (Vec<usize>, Vec<f32>) {
    let kept_indices = dedup_frames(frame_embeddings, dedup_threshold);

    let kept_embeddings: Vec<Vec<f32>> = kept_indices
        .iter()
        .map(|&i| frame_embeddings[i].clone())
        .collect();

    let video_embedding = mean_pool(&kept_embeddings);

    (kept_indices, video_embedding)
}

/// Run the re-embed worker loop: claim re_embed jobs, re-embed with new model, update catalog.
/// Returns the number of jobs processed.
pub fn run_reembed_worker(
    db: &StateDb,
    session: &EmbedSession,
    catalog: &Catalog,
    model_id: &str,
    model_version: &str,
    worker_id: &str,
) -> Result<u64> {
    let mut count = 0u64;

    while let Some(job) = db.claim(JobStage::ReEmbed, worker_id)? {
        let file_path = &job.file_path;

        // Check if file exists
        if !file_path.exists() {
            db.skip(job.id)?;
            continue;
        }

        match session.embed_image(file_path) {
            Ok(embedding) => {
                // Find existing catalog records for this file hash
                let existing = catalog.get_by_hash(&job.file_hash.0)?;

                if existing.is_empty() {
                    // No existing record — skip (file may not have been indexed yet)
                    db.skip(job.id)?;
                    continue;
                }

                // Update each matching record with the new embedding
                for mut record in existing {
                    record.embedding = embedding.clone();
                    record.model_id = model_id.to_string();
                    record.model_version = model_version.to_string();
                    record.updated_at = chrono::Utc::now().to_rfc3339();
                    catalog.upsert(&record)?;
                }

                db.complete(job.id)?;
                count += 1;
            }
            Err(e) => {
                db.fail(job.id, &e.to_string())?;
            }
        }
    }

    Ok(count)
}

fn determine_media_type(ext: &str) -> String {
    match ext {
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tiff" | "tif" | "webp" | "heic" | "heif"
        | "avif" => "image".to_string(),
        "mp4" | "mov" | "avi" | "mkv" | "wmv" | "flv" | "webm" | "m4v" | "mpg" | "mpeg" => {
            "video".to_string()
        }
        _ => "image".to_string(), // Default to image
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_determine_media_type_image() {
        assert_eq!(determine_media_type("jpg"), "image");
        assert_eq!(determine_media_type("png"), "image");
        assert_eq!(determine_media_type("heic"), "image");
    }

    #[test]
    fn test_determine_media_type_video() {
        assert_eq!(determine_media_type("mp4"), "video");
        assert_eq!(determine_media_type("mov"), "video");
        assert_eq!(determine_media_type("mkv"), "video");
    }

    #[test]
    fn test_aggregate_video_frames_all_similar() {
        let emb = vec![1.0, 0.0, 0.0];
        let norm = l2_normalize(&emb);
        let frames = vec![norm.clone(), norm.clone(), norm.clone()];

        let (kept, video_emb) = aggregate_video_frames(&frames, 0.95);
        assert_eq!(kept.len(), 1);
        assert_eq!(video_emb.len(), 3);
    }

    #[test]
    fn test_aggregate_video_frames_all_different() {
        let frames = vec![
            l2_normalize(&vec![1.0, 0.0, 0.0]),
            l2_normalize(&vec![0.0, 1.0, 0.0]),
            l2_normalize(&vec![0.0, 0.0, 1.0]),
        ];

        let (kept, video_emb) = aggregate_video_frames(&frames, 0.95);
        assert_eq!(kept.len(), 3);
        assert_eq!(video_emb.len(), 3);

        // Video embedding should be L2-normalized
        let norm: f32 = video_emb.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }
}
