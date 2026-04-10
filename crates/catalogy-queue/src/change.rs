use catalogy_core::{JobStage, MediaType, Result, ScannedFile};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::StateDb;

/// What kind of change was detected for a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileChangeKind {
    /// File is new (not seen before).
    New,
    /// File at same path has a different hash (content changed).
    Modified,
    /// Same hash exists at a different path (file was moved/renamed).
    Moved { old_path: PathBuf },
    /// File is unchanged since last scan.
    Unchanged,
    /// File was in the DB but no longer on disk.
    Deleted,
}

/// A detected change for a single file.
#[derive(Clone, Debug)]
pub struct FileChange {
    pub hash: String,
    pub path: PathBuf,
    pub kind: FileChangeKind,
    pub media_type: MediaType,
}

/// Compare scanned files against the state database to classify changes.
pub fn detect_changes(db: &StateDb, scanned: &[ScannedFile]) -> Result<Vec<FileChange>> {
    let db_files = db.get_all_active_files()?;

    // Build lookup maps from DB state
    let mut db_by_hash: HashMap<String, String> = HashMap::new(); // hash -> path
    let mut db_by_path: HashMap<String, String> = HashMap::new(); // path -> hash

    for f in &db_files {
        db_by_hash.insert(f.file_hash.clone(), f.file_path.clone());
        db_by_path.insert(f.file_path.clone(), f.file_hash.clone());
    }

    let mut changes = Vec::new();
    let mut seen_hashes: HashSet<String> = HashSet::new();

    for file in scanned {
        let hash = &file.hash.0;
        let path_str = file.path.to_string_lossy().to_string();
        seen_hashes.insert(hash.clone());

        let kind = match (db_by_hash.get(hash), db_by_path.get(&path_str)) {
            // Same hash, same path → unchanged
            (Some(db_path), _) if *db_path == path_str => FileChangeKind::Unchanged,

            // Same hash, different path → moved
            (Some(db_path), _) => FileChangeKind::Moved {
                old_path: PathBuf::from(db_path),
            },

            // Different hash at same path → modified
            (None, Some(_old_hash)) => FileChangeKind::Modified,

            // Not in DB at all → new
            (None, None) => FileChangeKind::New,
        };

        changes.push(FileChange {
            hash: hash.clone(),
            path: file.path.clone(),
            kind,
            media_type: file.media_type.clone(),
        });
    }

    // Detect deleted: files in DB but not in scanned set
    for f in &db_files {
        if !seen_hashes.contains(&f.file_hash) {
            changes.push(FileChange {
                hash: f.file_hash.clone(),
                path: PathBuf::from(&f.file_path),
                kind: FileChangeKind::Deleted,
                media_type: MediaType::Image, // doesn't matter for deleted
            });
        }
    }

    Ok(changes)
}

/// Apply detected changes to the state DB and enqueue appropriate jobs.
pub fn apply_changes_and_enqueue(db: &StateDb, changes: &[FileChange]) -> Result<ApplyResult> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut result = ApplyResult::default();

    for change in changes {
        let path_str = change.path.to_string_lossy().to_string();

        match &change.kind {
            FileChangeKind::New => {
                db.upsert_file(&change.hash, &path_str, 0, &now, &now)?;
                enqueue_all_stages(db, &change.hash, &path_str)?;
                result.new_files += 1;
            }
            FileChangeKind::Modified => {
                db.upsert_file(&change.hash, &path_str, 0, &now, &now)?;
                enqueue_all_stages(db, &change.hash, &path_str)?;
                result.modified_files += 1;
            }
            FileChangeKind::Moved { .. } => {
                db.update_file_path(&change.hash, &path_str, &now)?;
                db.update_job_paths(&change.hash, &path_str)?;
                result.moved_files += 1;
            }
            FileChangeKind::Deleted => {
                db.mark_file_deleted(&change.hash, &now)?;
                db.enqueue_tombstone(&change.hash, &path_str)?;
                result.deleted_files += 1;
            }
            FileChangeKind::Unchanged => {
                result.unchanged_files += 1;
            }
        }
    }

    Ok(result)
}

/// Result of applying changes.
#[derive(Clone, Debug, Default)]
pub struct ApplyResult {
    pub new_files: u64,
    pub modified_files: u64,
    pub moved_files: u64,
    pub deleted_files: u64,
    pub unchanged_files: u64,
}

/// Enqueue jobs for all pipeline stages (New/Modified files).
fn enqueue_all_stages(db: &StateDb, file_hash: &str, file_path: &str) -> Result<()> {
    db.enqueue(file_hash, file_path, JobStage::ExtractMetadata)?;
    db.enqueue(file_hash, file_path, JobStage::ExtractFrames)?;
    db.enqueue(file_hash, file_path, JobStage::Embed)?;
    db.enqueue(file_hash, file_path, JobStage::Index)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use catalogy_core::{FileHash, ScannedFile};
    use std::time::SystemTime;

    fn make_scanned(path: &str, hash: &str) -> ScannedFile {
        ScannedFile {
            path: PathBuf::from(path),
            hash: FileHash(hash.to_string()),
            size: 1024,
            modified: SystemTime::now(),
            media_type: MediaType::Image,
        }
    }

    #[test]
    fn test_detect_new_files() {
        let db = StateDb::open_in_memory().unwrap();
        let scanned = vec![make_scanned("/photos/a.jpg", "hash_a")];

        let changes = detect_changes(&db, &scanned).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, FileChangeKind::New);
    }

    #[test]
    fn test_detect_unchanged() {
        let db = StateDb::open_in_memory().unwrap();
        db.upsert_file(
            "hash_a",
            "/photos/a.jpg",
            1024,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();

        let scanned = vec![make_scanned("/photos/a.jpg", "hash_a")];
        let changes = detect_changes(&db, &scanned).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, FileChangeKind::Unchanged);
    }

    #[test]
    fn test_detect_modified() {
        let db = StateDb::open_in_memory().unwrap();
        db.upsert_file(
            "hash_old",
            "/photos/a.jpg",
            1024,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();

        let scanned = vec![make_scanned("/photos/a.jpg", "hash_new")];
        let changes = detect_changes(&db, &scanned).unwrap();
        assert_eq!(changes.len(), 2); // modified + deleted (old hash is gone)

        let modified = changes
            .iter()
            .find(|c| c.kind == FileChangeKind::Modified)
            .unwrap();
        assert_eq!(modified.hash, "hash_new");
    }

    #[test]
    fn test_detect_moved() {
        let db = StateDb::open_in_memory().unwrap();
        db.upsert_file(
            "hash_a",
            "/old/a.jpg",
            1024,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();

        let scanned = vec![make_scanned("/new/a.jpg", "hash_a")];
        let changes = detect_changes(&db, &scanned).unwrap();
        assert_eq!(changes.len(), 1);
        assert!(matches!(changes[0].kind, FileChangeKind::Moved { .. }));
    }

    #[test]
    fn test_detect_deleted() {
        let db = StateDb::open_in_memory().unwrap();
        db.upsert_file(
            "hash_a",
            "/photos/a.jpg",
            1024,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();

        let scanned: Vec<ScannedFile> = vec![];
        let changes = detect_changes(&db, &scanned).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, FileChangeKind::Deleted);
    }

    #[test]
    fn test_apply_new_files_creates_jobs() {
        let db = StateDb::open_in_memory().unwrap();
        let scanned = vec![
            make_scanned("/photos/a.jpg", "hash_a"),
            make_scanned("/photos/b.jpg", "hash_b"),
        ];

        let changes = detect_changes(&db, &scanned).unwrap();
        let result = apply_changes_and_enqueue(&db, &changes).unwrap();

        assert_eq!(result.new_files, 2);
        let stats = db.stats().unwrap();
        // 2 files × 4 stages = 8 pending jobs
        assert_eq!(stats.pending, 8);
    }

    #[test]
    fn test_apply_idempotent_rescan() {
        let db = StateDb::open_in_memory().unwrap();
        let scanned = vec![make_scanned("/photos/a.jpg", "hash_a")];

        // First scan
        let changes = detect_changes(&db, &scanned).unwrap();
        apply_changes_and_enqueue(&db, &changes).unwrap();

        // Second scan - same files
        let changes = detect_changes(&db, &scanned).unwrap();
        let result = apply_changes_and_enqueue(&db, &changes).unwrap();

        assert_eq!(result.unchanged_files, 1);
        assert_eq!(result.new_files, 0);

        // Still only 4 jobs (from first scan)
        let stats = db.stats().unwrap();
        assert_eq!(stats.pending, 4);
    }

    #[test]
    fn test_apply_moved_file() {
        let db = StateDb::open_in_memory().unwrap();

        // First scan
        let scanned1 = vec![make_scanned("/old/a.jpg", "hash_a")];
        let changes = detect_changes(&db, &scanned1).unwrap();
        apply_changes_and_enqueue(&db, &changes).unwrap();

        // Second scan - file moved
        let scanned2 = vec![make_scanned("/new/a.jpg", "hash_a")];
        let changes = detect_changes(&db, &scanned2).unwrap();
        let result = apply_changes_and_enqueue(&db, &changes).unwrap();

        assert_eq!(result.moved_files, 1);

        // Verify path updated
        let file = db.get_file_by_hash("hash_a").unwrap().unwrap();
        assert_eq!(file.file_path, "/new/a.jpg");
    }

    #[test]
    fn test_apply_deleted_file() {
        let db = StateDb::open_in_memory().unwrap();

        // First scan
        let scanned1 = vec![make_scanned("/photos/a.jpg", "hash_a")];
        let changes = detect_changes(&db, &scanned1).unwrap();
        apply_changes_and_enqueue(&db, &changes).unwrap();

        // Second scan - file gone
        let scanned2: Vec<ScannedFile> = vec![];
        let changes = detect_changes(&db, &scanned2).unwrap();
        let result = apply_changes_and_enqueue(&db, &changes).unwrap();

        assert_eq!(result.deleted_files, 1);

        let file = db.get_file_by_hash("hash_a").unwrap().unwrap();
        assert_eq!(file.status, "deleted");
    }

    #[test]
    fn test_all_five_states() {
        let db = StateDb::open_in_memory().unwrap();

        // Initial state: A, B, C exist
        let scanned1 = vec![
            make_scanned("/photos/a.jpg", "hash_a"),
            make_scanned("/photos/b.jpg", "hash_b"),
            make_scanned("/photos/c.jpg", "hash_c"),
        ];
        let changes = detect_changes(&db, &scanned1).unwrap();
        apply_changes_and_enqueue(&db, &changes).unwrap();

        // Now: A unchanged, B modified, C moved, D new, nothing → E deleted... wait,
        // we didn't have E. Let me redo with an initial set that includes something to delete.
        // Reset by creating a fresh db
        let db = StateDb::open_in_memory().unwrap();
        db.upsert_file(
            "hash_a",
            "/photos/a.jpg",
            100,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();
        db.upsert_file(
            "hash_b",
            "/photos/b.jpg",
            200,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();
        db.upsert_file(
            "hash_c",
            "/photos/c.jpg",
            300,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();
        db.upsert_file(
            "hash_d",
            "/photos/d.jpg",
            400,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();

        // Scan: A unchanged, B modified (new hash), C moved, D deleted, E new
        let scanned = vec![
            make_scanned("/photos/a.jpg", "hash_a"),       // Unchanged
            make_scanned("/photos/b.jpg", "hash_b_new"),   // Modified (same path, different hash)
            make_scanned("/photos/c_moved.jpg", "hash_c"), // Moved (same hash, different path)
            make_scanned("/photos/e.jpg", "hash_e"),       // New
        ];
        // hash_d is not in scanned → Deleted

        let changes = detect_changes(&db, &scanned).unwrap();

        let unchanged: Vec<_> = changes
            .iter()
            .filter(|c| c.kind == FileChangeKind::Unchanged)
            .collect();
        let new: Vec<_> = changes
            .iter()
            .filter(|c| c.kind == FileChangeKind::New)
            .collect();
        let modified: Vec<_> = changes
            .iter()
            .filter(|c| c.kind == FileChangeKind::Modified)
            .collect();
        let moved: Vec<_> = changes
            .iter()
            .filter(|c| matches!(c.kind, FileChangeKind::Moved { .. }))
            .collect();
        let deleted: Vec<_> = changes
            .iter()
            .filter(|c| c.kind == FileChangeKind::Deleted)
            .collect();

        assert_eq!(unchanged.len(), 1);
        assert_eq!(new.len(), 1);
        assert_eq!(modified.len(), 1);
        assert_eq!(moved.len(), 1);
        // hash_b (old) and hash_d are both deleted
        assert_eq!(deleted.len(), 2);
    }
}
