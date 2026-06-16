use catalogy_core::{CatalogyError, ExtractionConfig, JobStage, MediaType, Result};
use catalogy_queue::StateDb;
use std::path::{Path, PathBuf};

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
            scene_threshold: config.scene_threshold as f32,
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

    // Videos: extract frames into a temp dir (removed when this function
    // returns), then persist them so the downstream embed stage can read them.
    let (_temp_dir, frames) = extract_frames(
        file_path,
        strategy,
        max_dimension,
        None, // fps not known yet at this stage
        None, // duration not known yet
    )?;

    // Generate thumbnail from first frame (if any), while the temp frames exist.
    let thumbnail_path = if let Some(first_frame) = frames.first() {
        generate_thumbnail(&first_frame.path, thumb_dir, file_hash).ok()
    } else {
        None
    };

    // Copy frames out of the temp dir into a persistent per-video cache. The
    // embed stage reads these after the temp dir has been dropped.
    let persisted = persist_frames(&frames, thumb_dir, file_hash)?;

    // Store frame info (with persistent paths) for the downstream embed stage.
    let frame_count = persisted.len();
    store_frame_metadata(file_path, file_hash, &persisted, thumb_dir)?;

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

/// Copy extracted frames out of their temporary directory into a persistent
/// per-video cache at `<thumb_dir>/frames/<file_hash>/`.
///
/// `extract_frames` writes frames into a `TempDir` that is removed as soon as
/// the extraction call returns, so frames must be copied somewhere durable for
/// the downstream embed stage to read. Returns `FrameOutput`s pointing at the
/// persisted copies (same `frame_index`/`timestamp_ms`, new `path`).
fn persist_frames(
    frames: &[FrameOutput],
    thumb_dir: &Path,
    file_hash: &str,
) -> Result<Vec<FrameOutput>> {
    if frames.is_empty() {
        return Ok(Vec::new());
    }

    let frames_dir = thumb_dir.join("frames").join(file_hash);
    std::fs::create_dir_all(&frames_dir).map_err(|e| {
        CatalogyError::Extraction(format!("creating frames dir {}: {e}", frames_dir.display()))
    })?;

    let mut out = Vec::with_capacity(frames.len());
    for frame in frames {
        let file_name = frame.path.file_name().ok_or_else(|| {
            CatalogyError::Extraction(format!(
                "frame path has no file name: {}",
                frame.path.display()
            ))
        })?;
        let dest = frames_dir.join(file_name);
        std::fs::copy(&frame.path, &dest).map_err(|e| {
            CatalogyError::Extraction(format!("persisting frame {}: {e}", frame.path.display()))
        })?;
        out.push(FrameOutput {
            path: dest,
            frame_index: frame.frame_index,
            timestamp_ms: frame.timestamp_ms,
        });
    }

    Ok(out)
}

/// Read the frame metadata sidecar written by [`store_frame_metadata`].
///
/// Returns the frames recorded for `file_hash` (in `frame_index` order), or an
/// empty vec if no sidecar exists. The downstream embed stage uses this to
/// locate the persisted frames for a video.
pub fn read_frame_metadata(thumb_dir: &Path, file_hash: &str) -> Result<Vec<FrameOutput>> {
    let meta_path = thumb_dir.join(format!("{file_hash}.frames"));
    let content = match std::fs::read_to_string(&meta_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(CatalogyError::Extraction(format!(
                "reading frame metadata {}: {e}",
                meta_path.display()
            )))
        }
    };

    let mut frames = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Format: frame_index,timestamp_ms,path  (path may itself contain commas)
        let mut parts = line.splitn(3, ',');
        let frame_index = parts.next().and_then(|s| s.trim().parse::<u32>().ok());
        let timestamp_ms = parts.next().and_then(|s| s.trim().parse::<u64>().ok());
        let path = parts.next();
        if let (Some(frame_index), Some(timestamp_ms), Some(path)) =
            (frame_index, timestamp_ms, path)
        {
            frames.push(FrameOutput {
                path: PathBuf::from(path),
                frame_index,
                timestamp_ms,
            });
        }
    }

    Ok(frames)
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
                assert!((scene_threshold - 0.4_f32).abs() < f32::EPSILON);
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

    #[test]
    fn test_persist_frames_copies_out_of_temp() {
        let src = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();

        // Two fake frames in the (soon-to-be-dropped) source dir.
        let mut frames = Vec::new();
        for i in [0u32, 1] {
            let p = src.path().join(format!("frame_{i:06}.jpg"));
            std::fs::write(&p, format!("frame{i}")).unwrap();
            frames.push(FrameOutput {
                path: p,
                frame_index: i,
                timestamp_ms: i as u64 * 1000,
            });
        }

        let persisted = persist_frames(&frames, thumb_dir.path(), "hash123").unwrap();
        assert_eq!(persisted.len(), 2);
        // Persisted copies live under the thumb dir, not the source temp dir.
        for f in &persisted {
            assert!(f.path.exists());
            assert!(f.path.starts_with(thumb_dir.path()));
        }

        // Dropping the source dir must not invalidate the persisted frames.
        drop(src);
        assert!(persisted[0].path.exists());
    }

    #[test]
    fn test_frame_metadata_roundtrip() {
        let thumb_dir = tempfile::tempdir().unwrap();
        let frames = vec![
            FrameOutput {
                path: PathBuf::from("/frames/hash/frame_000000.jpg"),
                frame_index: 0,
                timestamp_ms: 0,
            },
            FrameOutput {
                path: PathBuf::from("/frames/hash/frame_000001.jpg"),
                frame_index: 1,
                timestamp_ms: 2500,
            },
        ];

        store_frame_metadata(Path::new("/video.mp4"), "hash", &frames, thumb_dir.path()).unwrap();
        let read = read_frame_metadata(thumb_dir.path(), "hash").unwrap();

        assert_eq!(read.len(), 2);
        assert_eq!(read[0].frame_index, 0);
        assert_eq!(read[0].timestamp_ms, 0);
        assert_eq!(read[1].frame_index, 1);
        assert_eq!(read[1].timestamp_ms, 2500);
        assert_eq!(read[1].path, PathBuf::from("/frames/hash/frame_000001.jpg"));
    }

    #[test]
    fn test_read_frame_metadata_missing_is_empty() {
        let thumb_dir = tempfile::tempdir().unwrap();
        let read = read_frame_metadata(thumb_dir.path(), "nonexistent").unwrap();
        assert!(read.is_empty());
    }
}
