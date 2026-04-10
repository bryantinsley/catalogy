use catalogy_core::{CatalogyError, ExtractedFrame, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Frame extraction strategy.
#[derive(Clone, Debug)]
pub enum ExtractionStrategy {
    /// Scene change detection with a max-interval floor.
    Adaptive {
        /// ffmpeg scene detection threshold (0.0–1.0, lower = more sensitive). Default: 0.3
        scene_threshold: f32,
        /// Maximum seconds between frames even if no scene change. Default: 60
        max_interval_seconds: u32,
    },
    /// Extract one frame every N seconds.
    Interval { seconds: u32 },
}

/// A frame extracted to disk.
#[derive(Clone, Debug)]
pub struct FrameOutput {
    pub path: PathBuf,
    pub frame_index: u32,
    pub timestamp_ms: u64,
}

const BATCH_SIZE: usize = 100;

/// Build ffmpeg command-line arguments for frame extraction.
///
/// Returns the argument list (excluding the `ffmpeg` binary name).
pub fn build_ffmpeg_args(
    video_path: &Path,
    output_pattern: &str,
    strategy: &ExtractionStrategy,
    max_dimension: u32,
) -> Vec<String> {
    let video = video_path.to_string_lossy().to_string();

    let select_filter = match strategy {
        ExtractionStrategy::Adaptive {
            scene_threshold,
            max_interval_seconds,
        } => {
            let fps = 1.0 / *max_interval_seconds as f64;
            format!(
                "select='gt(scene,{scene_threshold})+isnan(prev_selected_t)+gte(t-prev_selected_t,{max_interval_seconds})',fps={fps:.6}:round=up",
            )
        }
        ExtractionStrategy::Interval { seconds } => {
            let fps = 1.0 / *seconds as f64;
            format!("fps={fps:.6}")
        }
    };

    let scale_filter =
        format!("scale='if(gt(iw,ih),{max_dimension},-2)':'if(gt(iw,ih),-2,{max_dimension})'");

    let vf = format!("{select_filter},{scale_filter}");

    vec![
        "-i".to_string(),
        video,
        "-vf".to_string(),
        vf,
        "-vsync".to_string(),
        "vfr".to_string(),
        "-frame_pts".to_string(),
        "1".to_string(),
        "-q:v".to_string(),
        "2".to_string(),
        output_pattern.to_string(),
    ]
}

/// Parse extracted frame JPEG files from an output directory.
///
/// ffmpeg with `frame_pts 1` names files with PTS-based indices.
/// We sort by filename and assign sequential frame indices.
/// Timestamps are estimated from the frame number and video fps.
pub fn parse_frame_files(
    output_dir: &Path,
    video_fps: Option<f32>,
    video_duration_ms: Option<u64>,
) -> Result<Vec<FrameOutput>> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(output_dir)
        .map_err(|e| CatalogyError::Extraction(format!("reading output dir: {e}")))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "jpg"))
        .collect();

    entries.sort();

    let total_frames = entries.len();
    let mut frames = Vec::with_capacity(total_frames);

    for (idx, path) in entries.into_iter().enumerate() {
        // Try to extract PTS from filename (frame_NNNNNN.jpg)
        let timestamp_ms =
            estimate_timestamp(&path, idx, total_frames, video_fps, video_duration_ms);

        frames.push(FrameOutput {
            path,
            frame_index: idx as u32,
            timestamp_ms,
        });
    }

    Ok(frames)
}

/// Estimate frame timestamp from the filename PTS or evenly distribute across duration.
fn estimate_timestamp(
    path: &Path,
    idx: usize,
    total_frames: usize,
    video_fps: Option<f32>,
    video_duration_ms: Option<u64>,
) -> u64 {
    // Try to parse PTS from filename like "frame_000042.jpg" -> pts=42
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        if let Some(num_str) = stem.strip_prefix("frame_") {
            if let Ok(pts) = num_str.parse::<u64>() {
                // With frame_pts=1, the number is the PTS in timebase units.
                // For video with known fps, PTS / fps gives seconds.
                if let Some(fps) = video_fps {
                    if fps > 0.0 {
                        return (pts as f64 / fps as f64 * 1000.0) as u64;
                    }
                }
                // Fallback: treat PTS as milliseconds
                return pts;
            }
        }
    }

    // Fallback: evenly distribute across video duration
    if let Some(duration_ms) = video_duration_ms {
        if total_frames > 1 {
            return duration_ms * idx as u64 / (total_frames as u64 - 1);
        }
        return 0;
    }

    0
}

/// Extract frames from a video file using ffmpeg CLI.
///
/// Returns the extracted frames as file paths in a temporary directory.
/// The caller is responsible for the temp directory lifetime.
///
/// For long videos, frames are extracted in a single ffmpeg pass
/// (ffmpeg handles memory internally), but results are yielded in
/// batches to allow the caller to process incrementally.
pub fn extract_frames(
    video_path: &Path,
    strategy: &ExtractionStrategy,
    max_dimension: u32,
    video_fps: Option<f32>,
    video_duration_ms: Option<u64>,
) -> Result<(tempfile::TempDir, Vec<FrameOutput>)> {
    if !video_path.exists() {
        return Err(CatalogyError::FileNotFound {
            path: video_path.to_path_buf(),
        });
    }

    let temp_dir = tempfile::tempdir()
        .map_err(|e| CatalogyError::Extraction(format!("creating temp dir: {e}")))?;

    let output_pattern = temp_dir
        .path()
        .join("frame_%06d.jpg")
        .to_string_lossy()
        .to_string();

    let args = build_ffmpeg_args(video_path, &output_pattern, strategy, max_dimension);

    let output = Command::new("ffmpeg")
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("warning")
        .args(&args)
        .output()
        .map_err(|e| {
            CatalogyError::Extraction(format!(
                "ffmpeg not found or failed to execute: {e}. Is ffmpeg installed?"
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CatalogyError::Extraction(format!(
            "ffmpeg exited with {}: {}",
            output.status,
            stderr.trim()
        )));
    }

    let frames = parse_frame_files(temp_dir.path(), video_fps, video_duration_ms)?;

    Ok((temp_dir, frames))
}

/// Process extracted frames in batches, calling a callback for each batch.
/// Useful for memory-guarding long videos.
pub fn process_frames_in_batches<F>(
    frames: &[FrameOutput],
    source_video: &Path,
    mut callback: F,
) -> Result<Vec<ExtractedFrame>>
where
    F: FnMut(&[ExtractedFrame]) -> Result<()>,
{
    let mut all_frames = Vec::with_capacity(frames.len());

    for chunk in frames.chunks(BATCH_SIZE) {
        let mut batch = Vec::with_capacity(chunk.len());
        for frame in chunk {
            let image_data = std::fs::read(&frame.path).map_err(|e| {
                CatalogyError::Extraction(format!("reading frame {}: {e}", frame.path.display()))
            })?;
            batch.push(ExtractedFrame {
                source_video: source_video.to_path_buf(),
                frame_index: frame.frame_index,
                timestamp_ms: frame.timestamp_ms,
                image_data,
            });
        }
        callback(&batch)?;
        all_frames.extend(batch);
    }

    Ok(all_frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_ffmpeg_args_adaptive() {
        let args = build_ffmpeg_args(
            Path::new("/test/video.mp4"),
            "/tmp/frame_%06d.jpg",
            &ExtractionStrategy::Adaptive {
                scene_threshold: 0.3,
                max_interval_seconds: 60,
            },
            512,
        );

        assert_eq!(args[0], "-i");
        assert_eq!(args[1], "/test/video.mp4");
        assert_eq!(args[2], "-vf");
        // Check that the filter contains scene detection and scale
        let vf = &args[3];
        assert!(vf.contains("select="));
        assert!(vf.contains("gt(scene,0.3)"));
        assert!(vf.contains("prev_selected_t"));
        assert!(vf.contains("scale="));
        assert!(vf.contains("512"));
        assert_eq!(args[4], "-vsync");
        assert_eq!(args[5], "vfr");
        assert_eq!(args[6], "-frame_pts");
        assert_eq!(args[7], "1");
        assert_eq!(args[8], "-q:v");
        assert_eq!(args[9], "2");
        assert_eq!(args[10], "/tmp/frame_%06d.jpg");
    }

    #[test]
    fn test_build_ffmpeg_args_interval() {
        let args = build_ffmpeg_args(
            Path::new("/test/video.mp4"),
            "/tmp/frame_%06d.jpg",
            &ExtractionStrategy::Interval { seconds: 10 },
            512,
        );

        let vf = &args[3];
        assert!(vf.contains("fps=0.1"));
        assert!(vf.contains("scale="));
        assert!(vf.contains("512"));
        assert!(!vf.contains("scene"));
    }

    #[test]
    fn test_build_ffmpeg_args_interval_30s() {
        let args = build_ffmpeg_args(
            Path::new("/video.mp4"),
            "/out/frame_%06d.jpg",
            &ExtractionStrategy::Interval { seconds: 30 },
            256,
        );

        let vf = &args[3];
        // fps should be 1/30
        assert!(vf.contains("fps=0.0333"));
        assert!(vf.contains("256"));
    }

    #[test]
    fn test_estimate_timestamp_from_pts() {
        let path = PathBuf::from("/tmp/frame_000750.jpg");
        let ts = estimate_timestamp(&path, 0, 10, Some(25.0), Some(60000));
        // PTS 750 at 25fps = 30s = 30000ms
        assert_eq!(ts, 30000);
    }

    #[test]
    fn test_estimate_timestamp_no_fps() {
        let path = PathBuf::from("/tmp/frame_001000.jpg");
        let ts = estimate_timestamp(&path, 0, 10, None, Some(60000));
        // No fps, treat PTS as ms
        assert_eq!(ts, 1000);
    }

    #[test]
    fn test_estimate_timestamp_no_pts() {
        let path = PathBuf::from("/tmp/some_frame.jpg");
        let ts = estimate_timestamp(&path, 3, 10, None, Some(9000));
        // Evenly distribute: 9000 * 3 / 9 = 3000
        assert_eq!(ts, 3000);
    }

    #[test]
    fn test_parse_frame_files_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let frames = parse_frame_files(dir.path(), Some(30.0), Some(10000)).unwrap();
        assert!(frames.is_empty());
    }

    #[test]
    fn test_parse_frame_files_with_jpegs() {
        let dir = tempfile::tempdir().unwrap();

        // Create fake frame files
        for i in [1, 30, 60] {
            let name = format!("frame_{i:06}.jpg");
            std::fs::write(dir.path().join(name), b"fake jpeg").unwrap();
        }
        // Create a non-jpg file that should be ignored
        std::fs::write(dir.path().join("readme.txt"), b"ignore me").unwrap();

        let frames = parse_frame_files(dir.path(), Some(30.0), Some(60000)).unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].frame_index, 0);
        assert_eq!(frames[1].frame_index, 1);
        assert_eq!(frames[2].frame_index, 2);
        // PTS 1 at 30fps = 33ms
        assert_eq!(frames[0].timestamp_ms, 33);
        // PTS 30 at 30fps = 1000ms
        assert_eq!(frames[1].timestamp_ms, 1000);
        // PTS 60 at 30fps = 2000ms
        assert_eq!(frames[2].timestamp_ms, 2000);
    }

    #[test]
    fn test_extract_frames_missing_file() {
        let result = extract_frames(
            Path::new("/nonexistent/video.mp4"),
            &ExtractionStrategy::Interval { seconds: 10 },
            512,
            None,
            None,
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found") || err.contains("nonexistent"));
    }

    #[test]
    fn test_process_frames_in_batches() {
        let dir = tempfile::tempdir().unwrap();

        // Create 5 fake frame files
        for i in 0..5 {
            let name = format!("frame_{i:06}.jpg");
            std::fs::write(dir.path().join(&name), format!("data{i}").as_bytes()).unwrap();
        }

        let frame_outputs: Vec<FrameOutput> = (0..5)
            .map(|i| FrameOutput {
                path: dir.path().join(format!("frame_{i:06}.jpg")),
                frame_index: i as u32,
                timestamp_ms: i as u64 * 1000,
            })
            .collect();

        let mut batch_count = 0;
        let result =
            process_frames_in_batches(&frame_outputs, Path::new("/test/video.mp4"), |_batch| {
                batch_count += 1;
                Ok(())
            })
            .unwrap();

        assert_eq!(result.len(), 5);
        assert_eq!(batch_count, 1); // 5 < BATCH_SIZE(100), so one batch
        assert_eq!(result[0].source_video, PathBuf::from("/test/video.mp4"));
        assert_eq!(result[2].frame_index, 2);
        assert_eq!(result[2].timestamp_ms, 2000);
    }
}
