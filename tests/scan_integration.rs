use tempfile::TempDir;

/// End-to-end integration test: scan a temp directory, verify jobs created,
/// scan again with no changes, verify no new jobs.
#[test]
fn test_full_scan_pipeline() {
    let dir = TempDir::new().unwrap();

    // Create some test media files
    std::fs::write(dir.path().join("photo1.jpg"), b"jpeg content 1").unwrap();
    std::fs::write(dir.path().join("photo2.png"), b"png content 2").unwrap();
    std::fs::write(dir.path().join("video1.mp4"), b"mp4 content 3").unwrap();
    std::fs::write(dir.path().join("readme.txt"), b"not media").unwrap();

    // Create subdirectory with more files
    let subdir = dir.path().join("vacation");
    std::fs::create_dir(&subdir).unwrap();
    std::fs::write(subdir.join("sunset.jpg"), b"sunset jpeg data").unwrap();

    let image_exts: Vec<String> = vec!["jpg".into(), "jpeg".into(), "png".into()];
    let video_exts: Vec<String> = vec!["mp4".into(), "mov".into()];

    // Set up in-memory state database
    let db = catalogy_queue::StateDb::open_in_memory().unwrap();

    // ── First scan ──────────────────────────────────────────
    let scanned = catalogy_scanner::scan_directory(dir.path(), &image_exts, &video_exts).unwrap();
    assert_eq!(scanned.len(), 4); // 3 images + 1 video (not txt)

    let changes = catalogy_queue::detect_changes(&db, &scanned).unwrap();
    let result = catalogy_queue::apply_changes_and_enqueue(&db, &changes).unwrap();

    assert_eq!(result.new_files, 4);
    assert_eq!(result.modified_files, 0);
    assert_eq!(result.moved_files, 0);
    assert_eq!(result.deleted_files, 0);
    assert_eq!(result.unchanged_files, 0);

    // Should have 4 files × 4 stages = 16 pending jobs
    let stats = db.stats().unwrap();
    assert_eq!(stats.pending, 16);
    assert_eq!(stats.completed, 0);
    assert_eq!(stats.failed, 0);

    // Verify file count
    assert_eq!(db.file_count().unwrap(), 4);

    // ── Second scan (no changes) ────────────────────────────
    let scanned2 = catalogy_scanner::scan_directory(dir.path(), &image_exts, &video_exts).unwrap();
    assert_eq!(scanned2.len(), 4);

    let changes2 = catalogy_queue::detect_changes(&db, &scanned2).unwrap();
    let result2 = catalogy_queue::apply_changes_and_enqueue(&db, &changes2).unwrap();

    assert_eq!(result2.new_files, 0);
    assert_eq!(result2.unchanged_files, 4);
    assert_eq!(result2.modified_files, 0);
    assert_eq!(result2.moved_files, 0);
    assert_eq!(result2.deleted_files, 0);

    // Still only 16 jobs (no duplicates)
    let stats2 = db.stats().unwrap();
    assert_eq!(stats2.pending, 16);
}

/// Test that deleting a file is detected on rescan.
#[test]
fn test_scan_detects_deletion() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("keep.jpg"), b"keep me").unwrap();
    std::fs::write(dir.path().join("delete_me.jpg"), b"delete me").unwrap();

    let image_exts: Vec<String> = vec!["jpg".into()];
    let video_exts: Vec<String> = vec![];
    let db = catalogy_queue::StateDb::open_in_memory().unwrap();

    // First scan
    let scanned = catalogy_scanner::scan_directory(dir.path(), &image_exts, &video_exts).unwrap();
    let changes = catalogy_queue::detect_changes(&db, &scanned).unwrap();
    catalogy_queue::apply_changes_and_enqueue(&db, &changes).unwrap();
    assert_eq!(db.file_count().unwrap(), 2);

    // Delete a file
    std::fs::remove_file(dir.path().join("delete_me.jpg")).unwrap();

    // Second scan
    let scanned2 = catalogy_scanner::scan_directory(dir.path(), &image_exts, &video_exts).unwrap();
    assert_eq!(scanned2.len(), 1);

    let changes2 = catalogy_queue::detect_changes(&db, &scanned2).unwrap();
    let result2 = catalogy_queue::apply_changes_and_enqueue(&db, &changes2).unwrap();

    assert_eq!(result2.unchanged_files, 1);
    assert_eq!(result2.deleted_files, 1);
    // Only 1 active file now
    assert_eq!(db.file_count().unwrap(), 1);
}

/// Test that moving a file (same content, different path) is detected.
#[test]
fn test_scan_detects_move() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("original.jpg"), b"same content").unwrap();

    let image_exts: Vec<String> = vec!["jpg".into()];
    let video_exts: Vec<String> = vec![];
    let db = catalogy_queue::StateDb::open_in_memory().unwrap();

    // First scan
    let scanned = catalogy_scanner::scan_directory(dir.path(), &image_exts, &video_exts).unwrap();
    let changes = catalogy_queue::detect_changes(&db, &scanned).unwrap();
    catalogy_queue::apply_changes_and_enqueue(&db, &changes).unwrap();

    // Move the file (delete old, create new with same content)
    std::fs::remove_file(dir.path().join("original.jpg")).unwrap();
    std::fs::write(dir.path().join("renamed.jpg"), b"same content").unwrap();

    // Second scan
    let scanned2 = catalogy_scanner::scan_directory(dir.path(), &image_exts, &video_exts).unwrap();
    let changes2 = catalogy_queue::detect_changes(&db, &scanned2).unwrap();
    let result2 = catalogy_queue::apply_changes_and_enqueue(&db, &changes2).unwrap();

    assert_eq!(result2.moved_files, 1);
    assert_eq!(result2.new_files, 0);
    assert_eq!(result2.deleted_files, 0);
}

/// Test scan with file on disk using a persistent database.
#[test]
fn test_scan_with_persistent_db() {
    let media_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("state.db");

    std::fs::write(media_dir.path().join("test.jpg"), b"test data").unwrap();

    let image_exts: Vec<String> = vec!["jpg".into()];
    let video_exts: Vec<String> = vec![];

    // First session
    {
        let db = catalogy_queue::StateDb::open(&db_path).unwrap();
        let scanned =
            catalogy_scanner::scan_directory(media_dir.path(), &image_exts, &video_exts).unwrap();
        let changes = catalogy_queue::detect_changes(&db, &scanned).unwrap();
        catalogy_queue::apply_changes_and_enqueue(&db, &changes).unwrap();
    }

    // Second session (new db connection)
    {
        let db = catalogy_queue::StateDb::open(&db_path).unwrap();
        let scanned =
            catalogy_scanner::scan_directory(media_dir.path(), &image_exts, &video_exts).unwrap();
        let changes = catalogy_queue::detect_changes(&db, &scanned).unwrap();
        let result = catalogy_queue::apply_changes_and_enqueue(&db, &changes).unwrap();

        // Should be unchanged since data persisted
        assert_eq!(result.unchanged_files, 1);
        assert_eq!(result.new_files, 0);
    }
}
