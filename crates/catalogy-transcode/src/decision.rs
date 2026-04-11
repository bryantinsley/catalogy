use catalogy_core::{MediaMetadata, TranscodeConfig};

/// Result of the transcode decision engine.
#[derive(Clone, Debug)]
pub enum TranscodeDecision {
    Skip {
        reason: String,
    },
    Transcode {
        target_resolution: (u32, u32),
        target_codec: String,
        estimated_savings_bytes: i64,
    },
}

/// Parse a resolution string like "1080p", "720p", "4k" into (width, height).
pub fn parse_resolution(res: &str) -> Option<(u32, u32)> {
    match res.to_lowercase().as_str() {
        "4k" | "2160p" => Some((3840, 2160)),
        "1440p" | "2k" => Some((2560, 1440)),
        "1080p" => Some((1920, 1080)),
        "720p" => Some((1280, 720)),
        "480p" => Some((854, 480)),
        "360p" => Some((640, 360)),
        _ => None,
    }
}

/// Map codec names from ffprobe to normalized target codec names.
fn normalize_codec(codec: &str) -> &'static str {
    match codec.to_lowercase().as_str() {
        "hevc" | "h265" | "libx265" | "hevc_videotoolbox" => "h265",
        "h264" | "avc" | "libx264" | "h264_videotoolbox" => "h264",
        "av1" | "libaom-av1" | "libsvtav1" => "av1",
        _ => "other",
    }
}

/// Evaluate whether a video should be transcoded based on metadata and config.
pub fn should_transcode(metadata: &MediaMetadata, config: &TranscodeConfig) -> TranscodeDecision {
    if !config.enabled {
        return TranscodeDecision::Skip {
            reason: "transcoding disabled".to_string(),
        };
    }

    let (width, height) = match (metadata.width, metadata.height) {
        (Some(w), Some(h)) => (w, h),
        _ => {
            return TranscodeDecision::Skip {
                reason: "no resolution metadata available".to_string(),
            };
        }
    };

    let (_max_w, max_h) = match parse_resolution(&config.max_resolution) {
        Some(r) => r,
        None => {
            return TranscodeDecision::Skip {
                reason: format!("unknown max_resolution: {}", config.max_resolution),
            };
        }
    };

    let current_codec = metadata
        .codec
        .as_deref()
        .map(normalize_codec)
        .unwrap_or("unknown");
    let target_codec = normalize_codec(&config.target_codec);

    let needs_downscale = height > max_h;
    let needs_codec_change = current_codec != target_codec;

    if !needs_downscale && !needs_codec_change {
        return TranscodeDecision::Skip {
            reason: format!(
                "already within spec ({}x{}, codec={})",
                width, height, current_codec
            ),
        };
    }

    // Calculate target resolution
    let (target_w, target_h) = if needs_downscale {
        // Scale down preserving aspect ratio, height capped at max_h
        let scale = max_h as f64 / height as f64;
        let new_w = ((width as f64 * scale) as u32) & !1; // ensure even
        (new_w, max_h)
    } else {
        (width, height)
    };

    // Estimate savings
    let input_size = estimate_file_size(width, height, current_codec, metadata.bitrate_kbps);
    let output_size = estimate_file_size(target_w, target_h, target_codec, None);
    let estimated_savings = input_size as i64 - output_size as i64;

    TranscodeDecision::Transcode {
        target_resolution: (target_w, target_h),
        target_codec: config.target_codec.clone(),
        estimated_savings_bytes: estimated_savings,
    }
}

/// Rough file size estimate based on resolution and codec.
/// Uses typical bitrates for common resolutions and codecs.
fn estimate_file_size(width: u32, height: u32, codec: &str, actual_bitrate: Option<u32>) -> u64 {
    let pixels = width as u64 * height as u64;

    // If we have the actual bitrate and duration, use it
    if let Some(br) = actual_bitrate {
        // bitrate in kbps, assume 1 minute average for estimation
        return br as u64 * 1000 / 8 * 60;
    }

    // Estimate based on typical bitrates per pixel for each codec
    let bits_per_pixel = match codec {
        "h265" | "hevc" => 0.07,
        "h264" | "avc" => 0.12,
        "av1" => 0.06,
        _ => 0.15, // conservative for unknown codecs
    };

    // Assume 30fps, 60 seconds for a rough per-minute estimate
    let bitrate = pixels as f64 * bits_per_pixel * 30.0;
    (bitrate * 60.0 / 8.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(enabled: bool, max_res: &str, codec: &str) -> TranscodeConfig {
        TranscodeConfig {
            enabled,
            max_resolution: max_res.to_string(),
            target_codec: codec.to_string(),
            target_crf: 23,
            use_hw_encoder: true,
            original_policy: "keep".to_string(),
            staging_dir: "/tmp/staging".to_string(),
            archive_dir: None,
        }
    }

    fn make_metadata(width: u32, height: u32, codec: &str) -> MediaMetadata {
        MediaMetadata {
            width: Some(width),
            height: Some(height),
            duration_ms: Some(120_000),
            fps: Some(30.0),
            codec: Some(codec.to_string()),
            bitrate_kbps: Some(10_000),
            exif: None,
        }
    }

    #[test]
    fn test_parse_resolution() {
        assert_eq!(parse_resolution("1080p"), Some((1920, 1080)));
        assert_eq!(parse_resolution("720p"), Some((1280, 720)));
        assert_eq!(parse_resolution("4k"), Some((3840, 2160)));
        assert_eq!(parse_resolution("2160p"), Some((3840, 2160)));
        assert_eq!(parse_resolution("480p"), Some((854, 480)));
        assert!(parse_resolution("unknown").is_none());
    }

    #[test]
    fn test_disabled_config_skips() {
        let config = make_config(false, "1080p", "h265");
        let meta = make_metadata(3840, 2160, "h264");
        let decision = should_transcode(&meta, &config);
        match decision {
            TranscodeDecision::Skip { reason } => {
                assert!(reason.contains("disabled"));
            }
            _ => panic!("expected Skip"),
        }
    }

    #[test]
    fn test_no_metadata_skips() {
        let config = make_config(true, "1080p", "h265");
        let meta = MediaMetadata {
            width: None,
            height: None,
            duration_ms: Some(120_000),
            fps: Some(30.0),
            codec: Some("h264".to_string()),
            bitrate_kbps: None,
            exif: None,
        };
        let decision = should_transcode(&meta, &config);
        match decision {
            TranscodeDecision::Skip { reason } => {
                assert!(reason.contains("no resolution metadata"));
            }
            _ => panic!("expected Skip"),
        }
    }

    #[test]
    fn test_already_within_spec_skips() {
        let config = make_config(true, "1080p", "h265");
        let meta = make_metadata(1920, 1080, "hevc");
        let decision = should_transcode(&meta, &config);
        match decision {
            TranscodeDecision::Skip { reason } => {
                assert!(reason.contains("already within spec"));
            }
            _ => panic!("expected Skip"),
        }
    }

    #[test]
    fn test_720p_h265_within_1080p_spec() {
        let config = make_config(true, "1080p", "h265");
        let meta = make_metadata(1280, 720, "hevc");
        let decision = should_transcode(&meta, &config);
        match decision {
            TranscodeDecision::Skip { reason } => {
                assert!(reason.contains("already within spec"));
            }
            _ => panic!("expected Skip for 720p video within 1080p spec"),
        }
    }

    #[test]
    fn test_4k_h264_needs_transcode() {
        let config = make_config(true, "1080p", "h265");
        let meta = make_metadata(3840, 2160, "h264");
        let decision = should_transcode(&meta, &config);
        match decision {
            TranscodeDecision::Transcode {
                target_resolution,
                target_codec,
                estimated_savings_bytes,
            } => {
                assert_eq!(target_resolution.1, 1080);
                assert_eq!(target_codec, "h265");
                assert!(estimated_savings_bytes > 0);
            }
            _ => panic!("expected Transcode"),
        }
    }

    #[test]
    fn test_1080p_h264_needs_codec_change() {
        let config = make_config(true, "1080p", "h265");
        let meta = make_metadata(1920, 1080, "h264");
        let decision = should_transcode(&meta, &config);
        match decision {
            TranscodeDecision::Transcode {
                target_resolution,
                target_codec,
                ..
            } => {
                assert_eq!(target_resolution, (1920, 1080));
                assert_eq!(target_codec, "h265");
            }
            _ => panic!("expected Transcode for codec change"),
        }
    }

    #[test]
    fn test_unknown_resolution_config_skips() {
        let config = make_config(true, "ultrawide", "h265");
        let meta = make_metadata(3840, 2160, "h264");
        let decision = should_transcode(&meta, &config);
        match decision {
            TranscodeDecision::Skip { reason } => {
                assert!(reason.contains("unknown max_resolution"));
            }
            _ => panic!("expected Skip"),
        }
    }

    #[test]
    fn test_target_resolution_preserves_aspect_ratio() {
        let config = make_config(true, "1080p", "h265");
        // 4K ultrawide: 3440x1440
        let meta = make_metadata(3440, 1440, "h264");
        let decision = should_transcode(&meta, &config);
        match decision {
            TranscodeDecision::Transcode {
                target_resolution, ..
            } => {
                assert_eq!(target_resolution.1, 1080);
                // Width should maintain ~2.39:1 aspect ratio
                let ratio = target_resolution.0 as f64 / target_resolution.1 as f64;
                assert!((ratio - 2.39).abs() < 0.1);
            }
            _ => panic!("expected Transcode"),
        }
    }

    #[test]
    fn test_4k_prores_flagged_for_transcode() {
        let config = make_config(true, "1080p", "h265");
        let meta = make_metadata(3840, 2160, "prores");
        let decision = should_transcode(&meta, &config);
        assert!(matches!(decision, TranscodeDecision::Transcode { .. }));
    }

    #[test]
    fn test_normalize_codec_variants() {
        assert_eq!(normalize_codec("hevc"), "h265");
        assert_eq!(normalize_codec("h265"), "h265");
        assert_eq!(normalize_codec("HEVC"), "h265");
        assert_eq!(normalize_codec("h264"), "h264");
        assert_eq!(normalize_codec("avc"), "h264");
        assert_eq!(normalize_codec("av1"), "av1");
    }
}
