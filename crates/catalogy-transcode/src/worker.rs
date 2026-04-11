use catalogy_core::{JobStage, Result, TranscodeConfig};
use catalogy_queue::StateDb;

use crate::decision::{should_transcode, TranscodeDecision};
use crate::policy::apply_policy;
use crate::transcoder::{staging_output_path, transcode_video};
use crate::verify::verify_transcode;

const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "avi", "mkv", "wmv", "flv", "webm", "m4v", "mpg", "mpeg",
];

/// Dry-run report entry.
#[derive(Debug)]
pub struct DryRunEntry {
    pub file_path: String,
    pub file_size: i64,
    pub resolution: String,
    pub codec: String,
    pub decision: TranscodeDecision,
}

/// Run the transcode dry-run: evaluate all video files and print a report.
pub fn run_transcode_dry_run(db: &StateDb, config: &TranscodeConfig) -> Result<Vec<DryRunEntry>> {
    let video_files = db.get_video_files_with_metadata(VIDEO_EXTENSIONS)?;
    let mut entries = Vec::new();

    for (file, metadata) in &video_files {
        let resolution = match (metadata.width, metadata.height) {
            (Some(w), Some(h)) => format!("{}x{}", w, h),
            _ => "unknown".to_string(),
        };
        let codec = metadata
            .codec
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let decision = should_transcode(metadata, config);

        entries.push(DryRunEntry {
            file_path: file.file_path.clone(),
            file_size: file.file_size,
            resolution,
            codec,
            decision,
        });
    }

    Ok(entries)
}

/// Run the transcode worker: enqueue and process transcode jobs.
pub fn run_transcode_worker(
    db: &StateDb,
    config: &TranscodeConfig,
    worker_id: &str,
) -> Result<TranscodeStats> {
    let mut stats = TranscodeStats::default();

    // First, enqueue transcode jobs for eligible videos
    let video_files = db.get_video_files_with_metadata(VIDEO_EXTENSIONS)?;

    for (file, metadata) in &video_files {
        let decision = should_transcode(metadata, config);
        match decision {
            TranscodeDecision::Transcode { .. } => {
                db.enqueue(&file.file_hash, &file.file_path, JobStage::Transcode)?;
            }
            TranscodeDecision::Skip { .. } => {}
        }
    }

    // Find ffprobe for verification
    let ffprobe = catalogy_metadata::find_ffprobe(None);

    // Process transcode jobs
    while let Some(job) = db.claim(JobStage::Transcode, worker_id)? {
        let file_path = &job.file_path;

        // Get metadata for this file
        let metadata = match db.get_metadata(&job.file_hash.0)? {
            Some(m) => m,
            None => {
                db.fail(job.id, "no metadata available for transcode decision")?;
                stats.failed += 1;
                continue;
            }
        };

        // Re-evaluate decision (config may have changed)
        let decision = should_transcode(&metadata, config);

        match decision {
            TranscodeDecision::Skip { reason } => {
                db.skip(job.id)?;
                stats.skipped += 1;
                eprintln!("  Skip {}: {}", file_path.display(), reason);
            }
            TranscodeDecision::Transcode {
                target_codec,
                target_resolution,
                ..
            } => {
                let output_path = staging_output_path(file_path, &config.staging_dir);

                eprintln!(
                    "  Transcoding {} → {}x{} {} ...",
                    file_path.display(),
                    target_resolution.0,
                    target_resolution.1,
                    target_codec
                );

                // Execute transcode
                match transcode_video(file_path, &output_path, config) {
                    Ok(result) => {
                        // Verify transcode
                        let verify = verify_transcode(file_path, &output_path, ffprobe.as_deref())?;

                        if !verify.passed {
                            // Cleanup failed transcode output
                            let _ = std::fs::remove_file(&output_path);
                            let issues = verify.issues.join("; ");
                            db.fail(job.id, &format!("verification failed: {issues}"))?;
                            stats.failed += 1;
                            eprintln!("  FAILED verification: {issues}");
                            continue;
                        }

                        // Apply policy
                        match apply_policy(
                            file_path,
                            &output_path,
                            &config.original_policy,
                            config.archive_dir.as_deref(),
                        ) {
                            Ok(policy_result) => {
                                db.complete(job.id)?;
                                stats.completed += 1;
                                stats.total_savings_bytes += result.savings_bytes;
                                eprintln!(
                                    "  Done: saved {} bytes (policy={})",
                                    result.savings_bytes, policy_result.policy
                                );
                            }
                            Err(e) => {
                                db.fail(job.id, &format!("policy error: {e}"))?;
                                stats.failed += 1;
                                eprintln!("  FAILED policy: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        db.fail(job.id, &format!("transcode error: {e}"))?;
                        stats.failed += 1;
                        eprintln!("  FAILED transcode: {e}");
                    }
                }
            }
        }
    }

    Ok(stats)
}

/// Statistics from a transcode run.
#[derive(Debug, Default)]
pub struct TranscodeStats {
    pub completed: u64,
    pub skipped: u64,
    pub failed: u64,
    pub total_savings_bytes: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use catalogy_core::MediaMetadata;
    use catalogy_queue::StateDb;

    fn setup_db_with_video(db: &StateDb) {
        db.upsert_file(
            "hash_4k_h264",
            "/videos/big_video.mp4",
            1_000_000_000,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();

        db.store_metadata(
            "hash_4k_h264",
            &MediaMetadata {
                width: Some(3840),
                height: Some(2160),
                duration_ms: Some(120_000),
                fps: Some(30.0),
                codec: Some("h264".to_string()),
                bitrate_kbps: Some(50_000),
                exif: None,
            },
        )
        .unwrap();
    }

    fn setup_db_with_small_video(db: &StateDb) {
        db.upsert_file(
            "hash_720p_hevc",
            "/videos/small_video.mp4",
            50_000_000,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();

        db.store_metadata(
            "hash_720p_hevc",
            &MediaMetadata {
                width: Some(1280),
                height: Some(720),
                duration_ms: Some(60_000),
                fps: Some(30.0),
                codec: Some("hevc".to_string()),
                bitrate_kbps: Some(5_000),
                exif: None,
            },
        )
        .unwrap();
    }

    #[test]
    fn test_dry_run_flags_4k_h264() {
        let db = StateDb::open_in_memory().unwrap();
        setup_db_with_video(&db);

        let config = TranscodeConfig {
            enabled: true,
            max_resolution: "1080p".to_string(),
            target_codec: "h265".to_string(),
            target_crf: 23,
            use_hw_encoder: false,
            original_policy: "keep".to_string(),
            staging_dir: "/tmp/staging".to_string(),
            archive_dir: None,
        };

        let entries = run_transcode_dry_run(&db, &config).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(matches!(
            entries[0].decision,
            TranscodeDecision::Transcode { .. }
        ));
    }

    #[test]
    fn test_dry_run_skips_720p_hevc() {
        let db = StateDb::open_in_memory().unwrap();
        setup_db_with_small_video(&db);

        let config = TranscodeConfig {
            enabled: true,
            max_resolution: "1080p".to_string(),
            target_codec: "h265".to_string(),
            target_crf: 23,
            use_hw_encoder: false,
            original_policy: "keep".to_string(),
            staging_dir: "/tmp/staging".to_string(),
            archive_dir: None,
        };

        let entries = run_transcode_dry_run(&db, &config).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(matches!(
            entries[0].decision,
            TranscodeDecision::Skip { .. }
        ));
    }

    #[test]
    fn test_dry_run_disabled_skips_all() {
        let db = StateDb::open_in_memory().unwrap();
        setup_db_with_video(&db);
        setup_db_with_small_video(&db);

        let config = TranscodeConfig {
            enabled: false,
            max_resolution: "1080p".to_string(),
            target_codec: "h265".to_string(),
            target_crf: 23,
            use_hw_encoder: false,
            original_policy: "keep".to_string(),
            staging_dir: "/tmp/staging".to_string(),
            archive_dir: None,
        };

        let entries = run_transcode_dry_run(&db, &config).unwrap();
        assert!(entries
            .iter()
            .all(|e| matches!(e.decision, TranscodeDecision::Skip { .. })));
    }

    #[test]
    fn test_dry_run_no_videos() {
        let db = StateDb::open_in_memory().unwrap();

        let config = TranscodeConfig {
            enabled: true,
            max_resolution: "1080p".to_string(),
            target_codec: "h265".to_string(),
            target_crf: 23,
            use_hw_encoder: false,
            original_policy: "keep".to_string(),
            staging_dir: "/tmp/staging".to_string(),
            archive_dir: None,
        };

        let entries = run_transcode_dry_run(&db, &config).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_dry_run_with_image_files_ignored() {
        let db = StateDb::open_in_memory().unwrap();

        // Add an image file — should be ignored by transcode
        db.upsert_file(
            "hash_image",
            "/photos/sunset.jpg",
            5_000_000,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();

        db.store_metadata(
            "hash_image",
            &MediaMetadata {
                width: Some(4000),
                height: Some(3000),
                duration_ms: None,
                fps: None,
                codec: None,
                bitrate_kbps: None,
                exif: None,
            },
        )
        .unwrap();

        setup_db_with_video(&db);

        let config = TranscodeConfig {
            enabled: true,
            max_resolution: "1080p".to_string(),
            target_codec: "h265".to_string(),
            target_crf: 23,
            use_hw_encoder: false,
            original_policy: "keep".to_string(),
            staging_dir: "/tmp/staging".to_string(),
            archive_dir: None,
        };

        let entries = run_transcode_dry_run(&db, &config).unwrap();
        // Only the video should be in the report, not the image
        assert_eq!(entries.len(), 1);
        assert!(entries[0].file_path.contains("big_video"));
    }
}
