use catalogy_core::{CatalogyError, Result, TranscodeConfig};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::decision::parse_resolution;

/// Result of a transcode operation.
#[derive(Clone, Debug)]
pub struct TranscodeResult {
    pub output_path: PathBuf,
    pub output_size: u64,
    pub input_size: u64,
    pub savings_bytes: i64,
    pub duration_ms: u64,
}

/// Find the ffmpeg binary. Checks PATH.
pub fn find_ffmpeg() -> Option<PathBuf> {
    Command::new("which")
        .arg("ffmpeg")
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    Some(PathBuf::from(path))
                } else {
                    None
                }
            } else {
                None
            }
        })
}

/// Build the ffmpeg command arguments for transcoding.
pub fn build_ffmpeg_args(
    input_path: &Path,
    output_path: &Path,
    config: &TranscodeConfig,
) -> Vec<String> {
    let mut args = Vec::new();

    // Input
    args.push("-i".to_string());
    args.push(input_path.to_string_lossy().to_string());

    // Video codec
    let (video_codec, use_crf) = if config.use_hw_encoder {
        match config.target_codec.to_lowercase().as_str() {
            "h265" | "hevc" => ("hevc_videotoolbox".to_string(), false),
            "h264" | "avc" => ("h264_videotoolbox".to_string(), false),
            _ => (format!("lib{}", config.target_codec), true),
        }
    } else {
        match config.target_codec.to_lowercase().as_str() {
            "h265" | "hevc" => ("libx265".to_string(), true),
            "h264" | "avc" => ("libx264".to_string(), true),
            "av1" => ("libsvtav1".to_string(), true),
            other => (other.to_string(), true),
        }
    };

    args.push("-c:v".to_string());
    args.push(video_codec);

    // Quality setting
    if use_crf {
        args.push("-crf".to_string());
        args.push(config.target_crf.to_string());
    } else {
        // For VideoToolbox, use -q:v for quality (1-100, lower = better)
        // Map CRF roughly: CRF 18 → q:v 40, CRF 23 → q:v 55, CRF 28 → q:v 70
        let quality = ((config.target_crf as f32 - 18.0) / 10.0 * 30.0 + 40.0)
            .clamp(20.0, 85.0) as u32;
        args.push("-q:v".to_string());
        args.push(quality.to_string());
    }

    // Scale filter (only if max_resolution requires downscaling)
    if let Some((_, max_h)) = parse_resolution(&config.max_resolution) {
        args.push("-vf".to_string());
        args.push(format!("scale=-2:'min({},ih)'", max_h));
    }

    // Copy audio
    args.push("-c:a".to_string());
    args.push("copy".to_string());

    // Preserve metadata
    args.push("-map_metadata".to_string());
    args.push("0".to_string());

    // Overwrite output
    args.push("-y".to_string());

    // Output
    args.push(output_path.to_string_lossy().to_string());

    args
}

/// Transcode a video file using ffmpeg.
pub fn transcode_video(
    input_path: &Path,
    output_path: &Path,
    config: &TranscodeConfig,
) -> Result<TranscodeResult> {
    let ffmpeg = find_ffmpeg().ok_or_else(|| {
        CatalogyError::Transcode("ffmpeg not found on PATH".to_string())
    })?;

    let input_size = std::fs::metadata(input_path)
        .map_err(|e| CatalogyError::Transcode(format!("cannot read input file: {e}")))?
        .len();

    // Ensure output directory exists
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CatalogyError::Transcode(format!("cannot create output directory: {e}")))?;
    }

    let args = build_ffmpeg_args(input_path, output_path, config);

    let start = Instant::now();

    let output = Command::new(&ffmpeg)
        .args(&args)
        .output()
        .map_err(|e| CatalogyError::Transcode(format!("failed to run ffmpeg: {e}")))?;

    let duration_ms = start.elapsed().as_millis() as u64;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Check for hw encoder failure, could retry with software encoder
        if config.use_hw_encoder && stderr.contains("Error initializing") {
            return transcode_video_software_fallback(input_path, output_path, config);
        }
        return Err(CatalogyError::Transcode(format!(
            "ffmpeg failed (exit {}): {}",
            output.status,
            stderr.chars().take(500).collect::<String>()
        )));
    }

    let output_size = std::fs::metadata(output_path)
        .map_err(|e| CatalogyError::Transcode(format!("cannot read output file: {e}")))?
        .len();

    Ok(TranscodeResult {
        output_path: output_path.to_path_buf(),
        output_size,
        input_size,
        savings_bytes: input_size as i64 - output_size as i64,
        duration_ms,
    })
}

/// Fallback to software encoder if hardware encoder fails.
fn transcode_video_software_fallback(
    input_path: &Path,
    output_path: &Path,
    config: &TranscodeConfig,
) -> Result<TranscodeResult> {
    let sw_config = TranscodeConfig {
        use_hw_encoder: false,
        ..config.clone()
    };
    transcode_video(input_path, output_path, &sw_config)
}

/// Generate the output path for a transcoded file in the staging directory.
pub fn staging_output_path(input_path: &Path, staging_dir: &str) -> PathBuf {
    let file_stem = input_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let ext = "mp4"; // Transcode always outputs MP4

    let staging = PathBuf::from(staging_dir);
    staging.join(format!("{}_transcoded.{}", file_stem, ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(hw: bool, codec: &str, max_res: &str) -> TranscodeConfig {
        TranscodeConfig {
            enabled: true,
            max_resolution: max_res.to_string(),
            target_codec: codec.to_string(),
            target_crf: 23,
            use_hw_encoder: hw,
            original_policy: "keep".to_string(),
            staging_dir: "/tmp/staging".to_string(),
            archive_dir: None,
        }
    }

    #[test]
    fn test_build_ffmpeg_args_hw_h265() {
        let config = make_config(true, "h265", "1080p");
        let args = build_ffmpeg_args(
            Path::new("/input/video.mp4"),
            Path::new("/output/video.mp4"),
            &config,
        );

        assert!(args.contains(&"-i".to_string()));
        assert!(args.contains(&"/input/video.mp4".to_string()));
        assert!(args.contains(&"-c:v".to_string()));
        assert!(args.contains(&"hevc_videotoolbox".to_string()));
        assert!(args.contains(&"-q:v".to_string())); // hw encoder uses -q:v
        assert!(args.contains(&"-c:a".to_string()));
        assert!(args.contains(&"copy".to_string()));
        assert!(args.contains(&"-map_metadata".to_string()));
        assert!(args.contains(&"-y".to_string()));
    }

    #[test]
    fn test_build_ffmpeg_args_sw_h265() {
        let config = make_config(false, "h265", "1080p");
        let args = build_ffmpeg_args(
            Path::new("/input/video.mp4"),
            Path::new("/output/video.mp4"),
            &config,
        );

        assert!(args.contains(&"libx265".to_string()));
        assert!(args.contains(&"-crf".to_string()));
        assert!(args.contains(&"23".to_string()));
    }

    #[test]
    fn test_build_ffmpeg_args_sw_h264() {
        let config = make_config(false, "h264", "720p");
        let args = build_ffmpeg_args(
            Path::new("/input/video.mp4"),
            Path::new("/output/video.mp4"),
            &config,
        );

        assert!(args.contains(&"libx264".to_string()));
    }

    #[test]
    fn test_build_ffmpeg_args_hw_h264() {
        let config = make_config(true, "h264", "1080p");
        let args = build_ffmpeg_args(
            Path::new("/input/video.mp4"),
            Path::new("/output/video.mp4"),
            &config,
        );

        assert!(args.contains(&"h264_videotoolbox".to_string()));
    }

    #[test]
    fn test_build_ffmpeg_args_contains_scale_filter() {
        let config = make_config(false, "h265", "1080p");
        let args = build_ffmpeg_args(
            Path::new("/input/video.mp4"),
            Path::new("/output/video.mp4"),
            &config,
        );

        assert!(args.contains(&"-vf".to_string()));
        let vf_idx = args.iter().position(|a| a == "-vf").unwrap();
        assert!(args[vf_idx + 1].contains("1080"));
    }

    #[test]
    fn test_staging_output_path() {
        let path = staging_output_path(
            Path::new("/videos/my_video.mov"),
            "/tmp/staging",
        );
        assert_eq!(
            path,
            PathBuf::from("/tmp/staging/my_video_transcoded.mp4")
        );
    }

    #[test]
    fn test_staging_output_path_nested() {
        let path = staging_output_path(
            Path::new("/nas/media/vacation/clip.avi"),
            "/tmp/staging",
        );
        assert_eq!(
            path,
            PathBuf::from("/tmp/staging/clip_transcoded.mp4")
        );
    }
}
