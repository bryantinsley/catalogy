use catalogy_core::{CatalogyError, MediaMetadata, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Find the ffprobe binary. Checks configured path first, then PATH.
pub fn find_ffprobe(configured_path: Option<&str>) -> Option<PathBuf> {
    if let Some(path) = configured_path {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }

    // Try to find ffprobe on PATH
    Command::new("which")
        .arg("ffprobe")
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

/// Extract metadata from a video file using ffprobe.
pub fn extract_video_metadata(path: &Path, ffprobe_path: &Path) -> Result<MediaMetadata> {
    let output = Command::new(ffprobe_path)
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .map_err(|e| CatalogyError::Extraction(format!("Failed to run ffprobe: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CatalogyError::Extraction(format!(
            "ffprobe failed: {stderr}"
        )));
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    parse_ffprobe_output(&json_str)
}

/// Parse ffprobe JSON output into MediaMetadata.
pub fn parse_ffprobe_output(json: &str) -> Result<MediaMetadata> {
    let probe: FfprobeOutput =
        serde_json::from_str(json).map_err(|e| CatalogyError::Extraction(e.to_string()))?;

    let video_stream = probe
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"));

    let width = video_stream.and_then(|s| s.width);
    let height = video_stream.and_then(|s| s.height);
    let codec = video_stream.and_then(|s| s.codec_name.clone());
    let fps = video_stream.and_then(|s| parse_frame_rate(s.r_frame_rate.as_deref()));

    let duration_ms = probe
        .format
        .as_ref()
        .and_then(|f| f.duration.as_deref())
        .and_then(|d| d.parse::<f64>().ok())
        .map(|d| (d * 1000.0) as u64);

    let bitrate_kbps = probe
        .format
        .as_ref()
        .and_then(|f| f.bit_rate.as_deref())
        .and_then(|b| b.parse::<u64>().ok())
        .map(|b| (b / 1000) as u32);

    Ok(MediaMetadata {
        width,
        height,
        duration_ms,
        fps,
        codec,
        bitrate_kbps,
        exif: None,
    })
}

fn parse_frame_rate(rate: Option<&str>) -> Option<f32> {
    let rate = rate?;
    if let Some((num, denom)) = rate.split_once('/') {
        let n: f64 = num.parse().ok()?;
        let d: f64 = denom.parse().ok()?;
        if d == 0.0 {
            return None;
        }
        Some((n / d) as f32)
    } else {
        rate.parse().ok()
    }
}

// ── ffprobe JSON structures ──────────────────────────────

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    streams: Vec<FfprobeStream>,
    format: Option<FfprobeFormat>,
}

#[derive(Debug, Deserialize)]
struct FfprobeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    r_frame_rate: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
    bit_rate: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ffprobe_output() {
        let json = r#"{
            "streams": [
                {
                    "codec_type": "video",
                    "codec_name": "h264",
                    "width": 1920,
                    "height": 1080,
                    "r_frame_rate": "30000/1001"
                },
                {
                    "codec_type": "audio",
                    "codec_name": "aac"
                }
            ],
            "format": {
                "duration": "120.500000",
                "bit_rate": "5000000"
            }
        }"#;

        let meta = parse_ffprobe_output(json).unwrap();
        assert_eq!(meta.width, Some(1920));
        assert_eq!(meta.height, Some(1080));
        assert_eq!(meta.codec, Some("h264".to_string()));
        assert_eq!(meta.duration_ms, Some(120500));
        assert_eq!(meta.bitrate_kbps, Some(5000));

        let fps = meta.fps.unwrap();
        assert!((fps - 29.97).abs() < 0.01);
    }

    #[test]
    fn test_parse_ffprobe_no_video_stream() {
        let json = r#"{
            "streams": [
                {
                    "codec_type": "audio",
                    "codec_name": "aac"
                }
            ],
            "format": {
                "duration": "300.0",
                "bit_rate": "128000"
            }
        }"#;

        let meta = parse_ffprobe_output(json).unwrap();
        assert!(meta.width.is_none());
        assert!(meta.height.is_none());
        assert!(meta.codec.is_none());
        assert_eq!(meta.duration_ms, Some(300000));
    }

    #[test]
    fn test_parse_ffprobe_minimal() {
        let json = r#"{"streams": [], "format": {}}"#;
        let meta = parse_ffprobe_output(json).unwrap();
        assert!(meta.width.is_none());
        assert!(meta.duration_ms.is_none());
    }

    #[test]
    fn test_parse_ffprobe_invalid_json() {
        let result = parse_ffprobe_output("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_frame_rate() {
        assert!((parse_frame_rate(Some("30/1")).unwrap() - 30.0).abs() < 0.01);
        assert!((parse_frame_rate(Some("30000/1001")).unwrap() - 29.97).abs() < 0.01);
        assert!((parse_frame_rate(Some("24")).unwrap() - 24.0).abs() < 0.01);
        assert!(parse_frame_rate(Some("0/0")).is_none());
        assert!(parse_frame_rate(None).is_none());
    }

    #[test]
    fn test_find_ffprobe_nonexistent_path() {
        assert!(find_ffprobe(Some("/nonexistent/ffprobe")).is_none());
    }

    #[test]
    fn test_parse_ffprobe_real_world_format() {
        // Simulate a real ffprobe output with extra fields
        let json = r#"{
            "streams": [
                {
                    "index": 0,
                    "codec_name": "hevc",
                    "codec_long_name": "H.265 / HEVC",
                    "codec_type": "video",
                    "width": 3840,
                    "height": 2160,
                    "r_frame_rate": "60/1",
                    "avg_frame_rate": "60/1",
                    "pix_fmt": "yuv420p"
                }
            ],
            "format": {
                "filename": "test.mp4",
                "nb_streams": 1,
                "format_name": "mov,mp4,m4a,3gp,3g2,mj2",
                "duration": "10.000000",
                "size": "12500000",
                "bit_rate": "10000000"
            }
        }"#;

        let meta = parse_ffprobe_output(json).unwrap();
        assert_eq!(meta.width, Some(3840));
        assert_eq!(meta.height, Some(2160));
        assert_eq!(meta.codec, Some("hevc".to_string()));
        assert_eq!(meta.duration_ms, Some(10000));
        assert_eq!(meta.bitrate_kbps, Some(10000));
        assert!((meta.fps.unwrap() - 60.0).abs() < 0.01);
    }
}
