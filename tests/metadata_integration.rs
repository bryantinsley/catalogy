use catalogy_core::JobStage;
use catalogy_queue::StateDb;

/// Test parsing ffprobe output from a JSON fixture file.
#[test]
fn test_parse_ffprobe_fixture() {
    let fixture = include_str!("fixtures/ffprobe_sample.json");
    let meta = catalogy_metadata::parse_ffprobe_output(fixture).unwrap();

    assert_eq!(meta.width, Some(1920));
    assert_eq!(meta.height, Some(1080));
    assert_eq!(meta.codec, Some("h264".to_string()));
    assert_eq!(meta.duration_ms, Some(120120));
    assert_eq!(meta.bitrate_kbps, Some(4600));

    let fps = meta.fps.unwrap();
    assert!((fps - 29.97).abs() < 0.01);
}

/// Test extracting metadata from a PNG without EXIF data.
#[test]
fn test_extract_png_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.png");

    // Minimal 2x3 PNG
    let img = image::RgbImage::from_fn(2, 3, |_x, _y| image::Rgb([128, 64, 32]));
    img.save(&path).unwrap();

    let meta = catalogy_metadata::extract_image_metadata(&path).unwrap();
    assert_eq!(meta.width, Some(2));
    assert_eq!(meta.height, Some(3));
    assert!(meta.exif.is_none());
    assert!(meta.duration_ms.is_none());
}

/// Test the full metadata worker pipeline: enqueue → process → verify.
#[test]
fn test_metadata_worker_integration() {
    let db = StateDb::open_in_memory().unwrap();

    // Create test image files
    let dir = tempfile::tempdir().unwrap();

    // Create a few PNG files
    for i in 0..3 {
        let name = format!("image_{i}.png");
        let path = dir.path().join(&name);
        let img = image::RgbImage::from_fn(100 + i, 200 + i, |_x, _y| image::Rgb([128, 64, 32]));
        img.save(&path).unwrap();

        let path_str = path.to_str().unwrap();
        let hash = format!("hash_{i}");
        db.upsert_file(
            &hash,
            path_str,
            1000,
            "2024-01-01T00:00:00Z",
            "2024-01-01T00:00:00Z",
        )
        .unwrap();
        db.enqueue(&hash, path_str, JobStage::ExtractMetadata)
            .unwrap();
    }

    // Run the worker
    let processed = catalogy_metadata::run_metadata_worker(&db, None, false).unwrap();
    assert_eq!(processed, 3);

    // All jobs should be completed
    let stats = db.stats().unwrap();
    assert_eq!(stats.completed, 3);
    assert_eq!(stats.pending, 0);
    assert_eq!(stats.failed, 0);
}

/// Test that corrupt / missing files are handled gracefully.
#[test]
fn test_metadata_worker_handles_corrupt_files() {
    let db = StateDb::open_in_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();

    // Create a valid image
    let valid_path = dir.path().join("valid.png");
    let img = image::RgbImage::from_fn(10, 10, |_x, _y| image::Rgb([0, 0, 0]));
    img.save(&valid_path).unwrap();

    let valid_str = valid_path.to_str().unwrap();
    db.upsert_file(
        "valid_hash",
        valid_str,
        100,
        "2024-01-01T00:00:00Z",
        "2024-01-01T00:00:00Z",
    )
    .unwrap();
    db.enqueue("valid_hash", valid_str, JobStage::ExtractMetadata)
        .unwrap();

    // Enqueue a job for a video file that doesn't exist (ffprobe not available → fails)
    db.upsert_file(
        "video_hash",
        "/nonexistent/video.mp4",
        1000,
        "2024-01-01T00:00:00Z",
        "2024-01-01T00:00:00Z",
    )
    .unwrap();
    db.enqueue(
        "video_hash",
        "/nonexistent/video.mp4",
        JobStage::ExtractMetadata,
    )
    .unwrap();

    let processed = catalogy_metadata::run_metadata_worker(&db, None, false).unwrap();
    assert_eq!(processed, 2);

    let stats = db.stats().unwrap();
    // Valid image completes, video fails (no ffprobe)
    assert_eq!(stats.completed, 1);
    assert_eq!(stats.failed, 1);
}

/// Test that the worker correctly idempotently skips already-processed jobs.
#[test]
fn test_metadata_worker_idempotent() {
    let db = StateDb::open_in_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();

    let path = dir.path().join("test.png");
    let img = image::RgbImage::from_fn(50, 50, |_x, _y| image::Rgb([255, 0, 0]));
    img.save(&path).unwrap();

    let path_str = path.to_str().unwrap();
    db.upsert_file(
        "hash_a",
        path_str,
        100,
        "2024-01-01T00:00:00Z",
        "2024-01-01T00:00:00Z",
    )
    .unwrap();
    db.enqueue("hash_a", path_str, JobStage::ExtractMetadata)
        .unwrap();

    // First run processes the job
    let processed1 = catalogy_metadata::run_metadata_worker(&db, None, false).unwrap();
    assert_eq!(processed1, 1);

    // Second run finds nothing to do
    let processed2 = catalogy_metadata::run_metadata_worker(&db, None, false).unwrap();
    assert_eq!(processed2, 0);
}
