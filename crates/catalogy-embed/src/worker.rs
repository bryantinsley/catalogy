use catalogy_catalog::{Catalog, CatalogRecord};
use catalogy_core::{JobStage, Result};
use catalogy_queue::StateDb;

#[cfg(test)]
use crate::session::l2_normalize;
use crate::session::{dedup_frames, mean_pool, EmbedSession};

/// Run the embed worker loop: claim embed jobs, run inference, write to catalog.
/// Returns the number of jobs processed.
pub fn run_embed_worker(
    db: &StateDb,
    session: &EmbedSession,
    catalog: &Catalog,
    model_id: &str,
    model_version: &str,
    worker_id: &str,
) -> Result<u64> {
    let mut count = 0u64;

    loop {
        let job = match db.claim(JobStage::Embed, worker_id)? {
            Some(j) => j,
            None => break,
        };

        let file_path = &job.file_path;

        // Check if file exists
        if !file_path.exists() {
            db.skip(job.id)?;
            continue;
        }

        match session.embed_image(file_path) {
            Ok(embedding) => {
                // Get metadata from state DB if available
                let file_hash = &job.file_hash.0;
                let file_name = file_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let file_ext = file_path
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                let file_size = std::fs::metadata(file_path)
                    .map(|m| m.len() as i64)
                    .unwrap_or(0);

                // Determine media type from file extension
                let media_type = determine_media_type(&file_ext);

                let record = CatalogRecord {
                    id: uuid::Uuid::now_v7().to_string(),
                    file_hash: file_hash.clone(),
                    file_path: file_path.to_string_lossy().to_string(),
                    file_name,
                    file_size,
                    file_ext,
                    media_type,
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
                    source_video_path: None,
                    frame_index: None,
                    frame_timestamp_ms: None,
                    file_created: None,
                    file_modified: None,
                    indexed_at: chrono::Utc::now().to_rfc3339(),
                    updated_at: chrono::Utc::now().to_rfc3339(),
                    tombstone: false,
                };

                catalog.upsert(&record)?;
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

    loop {
        let job = match db.claim(JobStage::ReEmbed, worker_id)? {
            Some(j) => j,
            None => break,
        };

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
