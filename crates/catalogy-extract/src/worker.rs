use catalogy_core::{CatalogyError, ExtractionConfig, JobStage, MediaType, Result};
use catalogy_queue::StateDb;
use std::path::Path;

use crate::extract::{extract_frames, ExtractionStrategy, FrameOutput};
use crate::thumbnail::generate_thumbnail;

/// Result of processing an extract_frames job.
#[derive(Debug)]
pub struct ExtractFramesResult {
    pub frame_count: usize,
    pub thumbnail_path: Option<std::path::PathBuf>,
    pub skipped: bool,
}

/// Determine media type from file extension.
fn media_type_from_ext(path: &Path) -> MediaType {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tiff" | "tif" | "webp" | "heic" | "heif"
        | "avif" => MediaType::Image,
        _ => MediaType::Video,
    }
}

/// Build an ExtractionStrategy from config.
fn strategy_from_config(config: &ExtractionConfig) -> ExtractionStrategy {
    match config.frame_strategy.as_str() {
        "interval" => ExtractionStrategy::Interval {
            seconds: config.frame_interval_seconds,
        },
        _ => ExtractionStrategy::Adaptive {
            scene_threshold: config.scene_threshold,
            max_interval_seconds: config.max_interval_seconds,
        },
    }
}

/// Resolve a path with ~ expansion.
fn expand_tilde(path: &str) -> std::path::PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    std::path::PathBuf::from(path)
}

/// Process extract_frames jobs from the queue.
///
/// Returns the number of jobs processed (completed + skipped).
pub fn run_extract_frames_worker(
    db: &StateDb,
    config: &ExtractionConfig,
    worker_id: &str,
) -> Result<u32> {
    let strategy = strategy_from_config(config);
    let thumb_dir = expand_tilde(&config.thumbnail_dir);
    let mut processed = 0;

    while let Some(job) = db.claim(JobStage::ExtractFrames, worker_id)? {
        let result = process_single_job(
            &job.file_path,
            &job.file_hash.0,
            &strategy,
            config.frame_max_dimension,
            &thumb_dir,
        );

        match result {
            Ok(extract_result) => {
                if extract_result.skipped {
                    db.skip(job.id)?;
                } else {
                    db.complete(job.id)?;
                }
            }
            Err(e) => {
                db.fail(job.id, &e.to_string())?;
            }
        }
        processed += 1;
    }

    Ok(processed)
}

/// Process a single extract_frames job.
fn process_single_job(
    file_path: &Path,
    file_hash: &str,
    strategy: &ExtractionStrategy,
    max_dimension: u32,
    thumb_dir: &Path,
) -> Result<ExtractFramesResult> {
    let media_type = media_type_from_ext(file_path);

    // Images: skip frame extraction, just generate thumbnail
    if media_type == MediaType::Image {
        let thumb_result = generate_thumbnail(file_path, thumb_dir, file_hash);
        return Ok(ExtractFramesResult {
            frame_count: 0,
            thumbnail_path: thumb_result.ok(),
            skipped: true,
        });
    }

    // Videos: extract frames
    let (_temp_dir, frames) = extract_frames(
        file_path,
        strategy,
        max_dimension,
        None, // fps not known yet at this stage
        None, // duration not known yet
    )?;

    // Generate thumbnail from first frame (if any)
    let thumbnail_path = if let Some(first_frame) = frames.first() {
        generate_thumbnail(&first_frame.path, thumb_dir, file_hash).ok()
    } else {
        None
    };

    // Store frame info in job metadata for downstream embed stage
    let frame_count = frames.len();
    store_frame_metadata(file_path, file_hash, &frames, thumb_dir)?;

    Ok(ExtractFramesResult {
        frame_count,
        thumbnail_path,
        skipped: false,
    })
}

/// Store frame metadata as a simple JSON sidecar file in the thumbnail directory.
/// This is read by the downstream embed stage.
fn store_frame_metadata(
    _video_path: &Path,
    file_hash: &str,
    frames: &[FrameOutput],
    thumb_dir: &Path,
) -> Result<()> {
    if frames.is_empty() {
        return Ok(());
    }

    std::fs::create_dir_all(thumb_dir)
        .map_err(|e| CatalogyError::Extraction(format!("creating thumb dir: {e}")))?;

    // Store a simple line-delimited format: frame_index,timestamp_ms,path
    let meta_path = thumb_dir.join(format!("{file_hash}.frames"));
    let content: String = frames
        .iter()
        .map(|f| format!("{},{},{}", f.frame_index, f.timestamp_ms, f.path.display()))
        .collect::<Vec<_>>()
        .join("\n");

    std::fs::write(&meta_path, content).map_err(|e| {
        CatalogyError::Extraction(format!(
            "writing frame metadata {}: {e}",
            meta_path.display()
        ))
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_media_type_from_ext() {
        assert_eq!(
            media_type_from_ext(Path::new("/test/photo.jpg")),
            MediaType::Image
        );
        assert_eq!(
            media_type_from_ext(Path::new("/test/photo.PNG")),
            MediaType::Image
        );
        assert_eq!(
            media_type_from_ext(Path::new("/test/video.mp4")),
            MediaType::Video
        );
        assert_eq!(
            media_type_from_ext(Path::new("/test/video.mkv")),
            MediaType::Video
        );
    }

    #[test]
    fn test_strategy_from_config() {
        let config = ExtractionConfig {
            frame_strategy: "adaptive".to_string(),
            scene_threshold: 0.4,
            max_interval_seconds: 30,
            frame_interval_seconds: 10,
            frame_max_dimension: 512,
            dedup_similarity_threshold: 0.95,
            ffprobe_path: None,
            thumbnail_dir: "/tmp/thumbs".to_string(),
        };
        let strategy = strategy_from_config(&config);
        match strategy {
            ExtractionStrategy::Adaptive {
                scene_threshold,
                max_interval_seconds,
            } => {
                assert!((scene_threshold - 0.4).abs() < f32::EPSILON);
                assert_eq!(max_interval_seconds, 30);
            }
            _ => panic!("Expected Adaptive strategy"),
        }

        let config2 = ExtractionConfig {
            frame_strategy: "interval".to_string(),
            ..config
        };
        let strategy2 = strategy_from_config(&config2);
        match strategy2 {
            ExtractionStrategy::Interval { seconds } => {
                assert_eq!(seconds, 10);
            }
            _ => panic!("Expected Interval strategy"),
        }
    }

    #[test]
    fn test_expand_tilde() {
        let expanded = expand_tilde("~/some/path");
        assert!(!expanded.to_string_lossy().starts_with('~'));

        let absolute = expand_tilde("/absolute/path");
        assert_eq!(absolute, std::path::PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_process_single_job_image_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();

        // Create a test image
        let img = image::RgbImage::from_fn(100, 100, |_, _| image::Rgb([0, 0, 255]));
        let src_path = dir.path().join("test.jpg");
        img.save(&src_path).unwrap();

        let result = process_single_job(
            &src_path,
            "abc123",
            &ExtractionStrategy::Interval { seconds: 10 },
            512,
            thumb_dir.path(),
        )
        .unwrap();

        assert!(result.skipped);
        assert_eq!(result.frame_count, 0);
        assert!(result.thumbnail_path.is_some());
        assert!(result.thumbnail_path.unwrap().exists());
    }
}
