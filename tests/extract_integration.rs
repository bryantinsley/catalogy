use std::path::Path;
use std::process::Command;

/// Check if ffmpeg is available in PATH.
fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Generate a 2-second test video with scene changes using ffmpeg.
/// Creates a video with 3 distinct color segments to trigger scene detection.
fn generate_test_video(output_path: &Path) {
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            // 3 color segments: red (0-0.7s), green (0.7-1.4s), blue (1.4-2s)
            "color=c=red:s=320x240:d=0.7,format=yuv420p[r];\
             color=c=green:s=320x240:d=0.7,format=yuv420p[g];\
             color=c=blue:s=320x240:d=0.6,format=yuv420p[b];\
             [r][g][b]concat=n=3:v=1:a=0",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-t",
            "2",
        ])
        .arg(output_path.as_os_str())
        .output()
        .expect("ffmpeg should be available");

    assert!(
        status.status.success(),
        "ffmpeg failed to generate test video: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert!(output_path.exists(), "test video should exist");
}

#[test]
fn test_extract_frames_from_video_interval() {
    if !ffmpeg_available() {
        eprintln!("SKIPPING: ffmpeg not found in PATH");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let video_path = dir.path().join("test_video.mp4");
    generate_test_video(&video_path);

    let strategy = catalogy_extract::ExtractionStrategy::Interval { seconds: 1 };
    let (temp_dir, frames) =
        catalogy_extract::extract_frames(&video_path, &strategy, 512, Some(25.0), Some(2000))
            .unwrap();

    // With 1-second interval on a 2-second video, we expect ~2 frames
    assert!(
        !frames.is_empty(),
        "should extract at least one frame from a 2s video with 1s interval"
    );
    assert!(
        frames.len() <= 4,
        "should not extract more than 4 frames from a 2s video"
    );

    // Verify frame files exist and are valid JPEGs
    for frame in &frames {
        assert!(frame.path.exists(), "frame file should exist");
        let data = std::fs::read(&frame.path).unwrap();
        assert!(data.len() > 100, "frame should have substantial data");
        // JPEG files start with FF D8
        assert_eq!(data[0], 0xFF, "should be JPEG (FF D8 header)");
        assert_eq!(data[1], 0xD8, "should be JPEG (FF D8 header)");
    }

    // Verify frames are within the output dir
    for frame in &frames {
        assert!(
            frame.path.starts_with(temp_dir.path()),
            "frame should be in temp dir"
        );
    }
}

#[test]
fn test_extract_frames_from_video_adaptive() {
    if !ffmpeg_available() {
        eprintln!("SKIPPING: ffmpeg not found in PATH");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let video_path = dir.path().join("test_adaptive.mp4");
    generate_test_video(&video_path);

    let strategy = catalogy_extract::ExtractionStrategy::Adaptive {
        scene_threshold: 0.3,
        max_interval_seconds: 60,
    };
    let (_temp_dir, frames) =
        catalogy_extract::extract_frames(&video_path, &strategy, 256, Some(25.0), Some(2000))
            .unwrap();

    // With scene detection on a video with 3 color segments, we expect some frames
    // The exact count depends on ffmpeg's scene detection sensitivity
    assert!(
        !frames.is_empty(),
        "should extract at least one frame with scene detection"
    );

    // Verify frame files are valid
    for frame in &frames {
        assert!(frame.path.exists());
        let data = std::fs::read(&frame.path).unwrap();
        assert!(data.len() > 100);
    }
}

#[test]
fn test_thumbnail_from_extracted_frame() {
    if !ffmpeg_available() {
        eprintln!("SKIPPING: ffmpeg not found in PATH");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let thumb_dir = tempfile::tempdir().unwrap();
    let video_path = dir.path().join("test_thumb.mp4");
    generate_test_video(&video_path);

    // Extract frames
    let strategy = catalogy_extract::ExtractionStrategy::Interval { seconds: 1 };
    let (_temp_dir, frames) =
        catalogy_extract::extract_frames(&video_path, &strategy, 512, None, None).unwrap();

    assert!(!frames.is_empty(), "should have extracted frames");

    // Generate thumbnail from first frame
    let thumb_path =
        catalogy_extract::generate_thumbnail(&frames[0].path, thumb_dir.path(), "test_video_thumb")
            .unwrap();

    assert!(thumb_path.exists());

    // Verify it's a valid JPEG
    let thumb_data = std::fs::read(&thumb_path).unwrap();
    assert_eq!(thumb_data[0], 0xFF);
    assert_eq!(thumb_data[1], 0xD8);

    // Verify dimensions are within 300px
    let img = image::open(&thumb_path).unwrap();
    let (w, h) = image::GenericImageView::dimensions(&img);
    assert!(w <= 300 && h <= 300, "thumbnail should be within 300px");
}

#[test]
fn test_extract_frames_worker_with_image() {
    // Test that the worker correctly skips images
    let db = catalogy_queue::StateDb::open_in_memory().unwrap();

    // Create a test image file
    let dir = tempfile::tempdir().unwrap();
    let img = image::RgbImage::from_fn(200, 100, |_, _| image::Rgb([255, 128, 0]));
    let img_path = dir.path().join("photo.jpg");
    img.save(&img_path).unwrap();

    let img_path_str = img_path.to_string_lossy().to_string();

    // Set up DB: insert file, enqueue extract_frames job
    db.upsert_file(
        "hash_img",
        &img_path_str,
        1000,
        "2024-01-01T00:00:00Z",
        "2024-06-01T00:00:00Z",
    )
    .unwrap();
    db.enqueue(
        "hash_img",
        &img_path_str,
        catalogy_core::JobStage::ExtractFrames,
    )
    .unwrap();

    let thumb_dir = tempfile::tempdir().unwrap();
    let config = catalogy_core::ExtractionConfig {
        frame_strategy: "adaptive".to_string(),
        scene_threshold: 0.3,
        max_interval_seconds: 60,
        frame_interval_seconds: 30,
        frame_max_dimension: 512,
        dedup_similarity_threshold: 0.95,
        ffprobe_path: None,
        thumbnail_dir: thumb_dir.path().to_string_lossy().to_string(),
    };

    let count = catalogy_extract::run_extract_frames_worker(&db, &config, "test-worker").unwrap();
    assert_eq!(count, 1, "should process one job");

    // Verify the job was marked as skipped
    let stats = db.stats().unwrap();
    assert_eq!(stats.skipped, 1, "image job should be skipped");
    assert_eq!(stats.completed, 0);
    assert_eq!(stats.pending, 0);

    // Verify thumbnail was generated
    let thumb_path = thumb_dir.path().join("hash_img.jpg");
    assert!(
        thumb_path.exists(),
        "thumbnail should be generated for image"
    );
}

#[test]
fn test_extract_frames_worker_with_video() {
    if !ffmpeg_available() {
        eprintln!("SKIPPING: ffmpeg not found in PATH");
        return;
    }

    let db = catalogy_queue::StateDb::open_in_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let video_path = dir.path().join("test.mp4");
    generate_test_video(&video_path);

    let video_path_str = video_path.to_string_lossy().to_string();

    db.upsert_file(
        "hash_vid",
        &video_path_str,
        50000,
        "2024-01-01T00:00:00Z",
        "2024-06-01T00:00:00Z",
    )
    .unwrap();
    db.enqueue(
        "hash_vid",
        &video_path_str,
        catalogy_core::JobStage::ExtractFrames,
    )
    .unwrap();

    let thumb_dir = tempfile::tempdir().unwrap();
    let config = catalogy_core::ExtractionConfig {
        frame_strategy: "interval".to_string(),
        scene_threshold: 0.3,
        max_interval_seconds: 60,
        frame_interval_seconds: 1,
        frame_max_dimension: 512,
        dedup_similarity_threshold: 0.95,
        ffprobe_path: None,
        thumbnail_dir: thumb_dir.path().to_string_lossy().to_string(),
    };

    let count = catalogy_extract::run_extract_frames_worker(&db, &config, "test-worker").unwrap();
    assert_eq!(count, 1);

    let stats = db.stats().unwrap();
    assert_eq!(stats.completed, 1, "video job should be completed");
    assert_eq!(stats.skipped, 0);

    // Verify thumbnail was generated
    let thumb_path = thumb_dir.path().join("hash_vid.jpg");
    assert!(
        thumb_path.exists(),
        "thumbnail should be generated for video"
    );
}
