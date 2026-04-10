use catalogy_core::{CatalogyError, Result};
use std::path::Path;
use std::process::Command;

/// Result of transcode verification.
#[derive(Clone, Debug)]
pub struct VerifyResult {
    pub passed: bool,
    pub issues: Vec<String>,
}

/// Verify that a transcoded file is valid by comparing against the original.
///
/// Checks:
/// - Output file exists and is > 0 bytes
/// - Output is smaller than input (otherwise transcode was counterproductive)
/// - Duration within 0.5s of original
/// - Audio stream count matches
pub fn verify_transcode(
    original_path: &Path,
    transcoded_path: &Path,
    ffprobe_path: Option<&Path>,
) -> Result<VerifyResult> {
    let mut issues = Vec::new();

    // Check output file exists and has content
    let transcoded_meta = match std::fs::metadata(transcoded_path) {
        Ok(m) => m,
        Err(e) => {
            return Ok(VerifyResult {
                passed: false,
                issues: vec![format!("transcoded file not found: {e}")],
            });
        }
    };

    if transcoded_meta.len() == 0 {
        return Ok(VerifyResult {
            passed: false,
            issues: vec!["transcoded file is empty (0 bytes)".to_string()],
        });
    }

    // Check output is smaller than input
    let original_size = std::fs::metadata(original_path)
        .map_err(|e| CatalogyError::Transcode(format!("cannot read original: {e}")))?
        .len();

    if transcoded_meta.len() >= original_size {
        issues.push(format!(
            "transcoded file ({} bytes) is not smaller than original ({} bytes)",
            transcoded_meta.len(),
            original_size
        ));
    }

    // If we have ffprobe, do deeper checks
    if let Some(ffprobe) = ffprobe_path {
        if let Ok(original_info) = probe_media(original_path, ffprobe) {
            if let Ok(transcoded_info) = probe_media(transcoded_path, ffprobe) {
                // Check duration
                if let (Some(orig_dur), Some(trans_dur)) =
                    (original_info.duration_secs, transcoded_info.duration_secs)
                {
                    let diff = (orig_dur - trans_dur).abs();
                    if diff > 0.5 {
                        issues.push(format!(
                            "duration mismatch: original={:.2}s, transcoded={:.2}s (diff={:.2}s)",
                            orig_dur, trans_dur, diff
                        ));
                    }
                }

                // Check audio stream count
                if original_info.audio_stream_count != transcoded_info.audio_stream_count {
                    issues.push(format!(
                        "audio stream count mismatch: original={}, transcoded={}",
                        original_info.audio_stream_count, transcoded_info.audio_stream_count
                    ));
                }
            } else {
                issues.push("failed to probe transcoded file".to_string());
            }
        }
    }

    Ok(VerifyResult {
        passed: issues.is_empty(),
        issues,
    })
}

/// Basic media info from ffprobe for verification.
#[derive(Debug)]
struct ProbeInfo {
    duration_secs: Option<f64>,
    audio_stream_count: usize,
}

/// Probe a media file using ffprobe.
fn probe_media(path: &Path, ffprobe_path: &Path) -> Result<ProbeInfo> {
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
        .map_err(|e| CatalogyError::Transcode(format!("ffprobe failed: {e}")))?;

    if !output.status.success() {
        return Err(CatalogyError::Transcode("ffprobe returned error".to_string()));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| CatalogyError::Transcode(format!("invalid ffprobe json: {e}")))?;

    let duration_secs = json
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|d| d.as_str())
        .and_then(|d| d.parse::<f64>().ok());

    let audio_stream_count = json
        .get("streams")
        .and_then(|s| s.as_array())
        .map(|streams| {
            streams
                .iter()
                .filter(|s| s.get("codec_type").and_then(|t| t.as_str()) == Some("audio"))
                .count()
        })
        .unwrap_or(0);

    Ok(ProbeInfo {
        duration_secs,
        audio_stream_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_nonexistent_transcoded_file() {
        let result = verify_transcode(
            Path::new("/some/original.mp4"),
            Path::new("/nonexistent/transcoded.mp4"),
            None,
        )
        .unwrap();

        assert!(!result.passed);
        assert!(result.issues[0].contains("not found"));
    }

    #[test]
    fn test_verify_empty_transcoded_file() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.mp4");
        let transcoded = dir.path().join("transcoded.mp4");

        std::fs::write(&original, b"original content here for testing").unwrap();
        std::fs::write(&transcoded, b"").unwrap(); // empty file

        let result = verify_transcode(&original, &transcoded, None).unwrap();

        assert!(!result.passed);
        assert!(result.issues[0].contains("empty"));
    }

    #[test]
    fn test_verify_larger_transcoded_file() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.mp4");
        let transcoded = dir.path().join("transcoded.mp4");

        std::fs::write(&original, b"small").unwrap();
        std::fs::write(&transcoded, b"this is much larger content").unwrap();

        let result = verify_transcode(&original, &transcoded, None).unwrap();

        assert!(!result.passed);
        assert!(result.issues[0].contains("not smaller"));
    }

    #[test]
    fn test_verify_smaller_transcoded_file_passes_without_ffprobe() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original.mp4");
        let transcoded = dir.path().join("transcoded.mp4");

        std::fs::write(&original, b"this is the larger original content here!!").unwrap();
        std::fs::write(&transcoded, b"smaller output").unwrap();

        let result = verify_transcode(&original, &transcoded, None).unwrap();

        assert!(result.passed);
        assert!(result.issues.is_empty());
    }
}
