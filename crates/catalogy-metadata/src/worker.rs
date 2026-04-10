use catalogy_core::{JobStage, MediaType, Result};
use catalogy_queue::StateDb;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::Path;

use crate::image_metadata::extract_image_metadata;
use crate::video_metadata::extract_video_metadata;

/// Run the metadata extraction worker loop.
///
/// Claims `extract_metadata` jobs from the queue, extracts metadata,
/// stores results in the metadata side table, and marks jobs complete.
///
/// Returns the number of jobs processed.
pub fn run_metadata_worker(
    db: &StateDb,
    ffprobe_path: Option<&Path>,
    show_progress: bool,
) -> Result<u64> {
    let worker_id = format!("metadata-{}", std::process::id());
    let mut processed = 0u64;

    // Count pending jobs for progress bar
    let stats = db.stats()?;
    let pending_metadata = stats
        .by_stage
        .iter()
        .find(|(stage, ..)| stage == "extract_metadata")
        .map(|(_, p, r, ..)| p + r)
        .unwrap_or(0);

    let pb = if show_progress && pending_metadata > 0 {
        let pb = ProgressBar::new(pending_metadata);
        pb.set_style(
            ProgressStyle::with_template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({per_sec}) {msg}",
            )
            .unwrap()
            .progress_chars("#>-"),
        );
        pb.set_message("Extracting metadata...");
        Some(pb)
    } else {
        None
    };

    while let Some(job) = db.claim(JobStage::ExtractMetadata, &worker_id)? {
        let path = &job.file_path;
        let file_hash = &job.file_hash.0;

        // Determine media type from extension
        let media_type = classify_media_type(path);

        let result = match media_type {
            MediaType::Image => extract_image_metadata(path),
            MediaType::Video => {
                if let Some(fp) = ffprobe_path {
                    extract_video_metadata(path, fp)
                } else {
                    Err(catalogy_core::CatalogyError::Extraction(
                        "ffprobe not available".to_string(),
                    ))
                }
            }
            MediaType::VideoFrame => {
                // Video frames shouldn't have extract_metadata jobs
                db.complete(job.id)?;
                processed += 1;
                if let Some(ref pb) = pb {
                    pb.inc(1);
                }
                continue;
            }
        };

        match result {
            Ok(metadata) => {
                db.store_metadata(file_hash, &metadata)?;
                db.complete(job.id)?;
            }
            Err(e) => {
                let msg = format!("{e}");
                if let Some(ref pb) = pb {
                    pb.set_message(format!("Failed: {}", path.display()));
                }
                db.fail(job.id, &msg)?;
            }
        }

        processed += 1;
        if let Some(ref pb) = pb {
            pb.inc(1);
        }
    }

    if let Some(pb) = pb {
        pb.finish_with_message(format!("Metadata extraction complete: {processed} files"));
    }

    Ok(processed)
}

/// Classify a file path as Image or Video based on extension.
fn classify_media_type(path: &Path) -> MediaType {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tiff" | "tif" | "webp" | "heic" | "heif"
        | "avif" => MediaType::Image,
        "mp4" | "mov" | "avi" | "mkv" | "wmv" | "flv" | "webm" | "m4v" | "mpg" | "mpeg" => {
            MediaType::Video
        }
        _ => MediaType::Image, // fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_media_type() {
        assert_eq!(
            classify_media_type(Path::new("/test/photo.jpg")),
            MediaType::Image
        );
        assert_eq!(
            classify_media_type(Path::new("/test/photo.JPEG")),
            MediaType::Image
        );
        assert_eq!(
            classify_media_type(Path::new("/test/video.mp4")),
            MediaType::Video
        );
        assert_eq!(
            classify_media_type(Path::new("/test/video.MKV")),
            MediaType::Video
        );
        assert_eq!(
            classify_media_type(Path::new("/test/photo.png")),
            MediaType::Image
        );
    }

    #[test]
    fn test_worker_no_jobs() {
        let db = StateDb::open_in_memory().unwrap();
        let processed = run_metadata_worker(&db, None, false).unwrap();
        assert_eq!(processed, 0);
    }

    #[test]
    fn test_worker_processes_image_job() {
        let db = StateDb::open_in_memory().unwrap();

        // Create a minimal PNG test file
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.png");
        let png_data: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21, 0xBC,
            0x33, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        std::fs::write(&path, png_data).unwrap();

        let path_str = path.to_str().unwrap();
        db.upsert_file(
            "hash1",
            path_str,
            100,
            "2024-01-01T00:00:00Z",
            "2024-01-01T00:00:00Z",
        )
        .unwrap();
        db.enqueue("hash1", path_str, JobStage::ExtractMetadata)
            .unwrap();

        let processed = run_metadata_worker(&db, None, false).unwrap();
        assert_eq!(processed, 1);

        // Verify job is completed
        let stats = db.stats().unwrap();
        assert_eq!(stats.completed, 1);
        assert_eq!(stats.pending, 0);
    }

    #[test]
    fn test_worker_handles_missing_file() {
        let db = StateDb::open_in_memory().unwrap();

        db.upsert_file(
            "hash_missing",
            "/nonexistent/file.jpg",
            100,
            "2024-01-01T00:00:00Z",
            "2024-01-01T00:00:00Z",
        )
        .unwrap();
        db.enqueue(
            "hash_missing",
            "/nonexistent/file.jpg",
            JobStage::ExtractMetadata,
        )
        .unwrap();

        let processed = run_metadata_worker(&db, None, false).unwrap();
        assert_eq!(processed, 1);

        // The job should still complete (extract_image_metadata returns Ok with None fields)
        let stats = db.stats().unwrap();
        assert_eq!(stats.completed, 1);
    }

    #[test]
    fn test_worker_video_without_ffprobe() {
        let db = StateDb::open_in_memory().unwrap();

        db.upsert_file(
            "hash_vid",
            "/test/video.mp4",
            1000,
            "2024-01-01T00:00:00Z",
            "2024-01-01T00:00:00Z",
        )
        .unwrap();
        db.enqueue("hash_vid", "/test/video.mp4", JobStage::ExtractMetadata)
            .unwrap();

        // No ffprobe available → job should fail
        let processed = run_metadata_worker(&db, None, false).unwrap();
        assert_eq!(processed, 1);

        let stats = db.stats().unwrap();
        assert_eq!(stats.failed, 1);
    }
}
