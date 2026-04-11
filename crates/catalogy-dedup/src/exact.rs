use catalogy_core::Result;
use catalogy_queue::StateDb;
use serde::Serialize;

/// A set of files sharing the same content hash.
#[derive(Clone, Debug, Serialize)]
pub struct DuplicateSet {
    pub file_hash: String,
    pub files: Vec<DuplicateFile>,
}

/// A file within a duplicate set.
#[derive(Clone, Debug, Serialize)]
pub struct DuplicateFile {
    pub path: String,
    pub size: u64,
    pub modified: String,
}

/// Find exact duplicates by querying the files table for file_hash values
/// that appear more than once with status='active'.
pub fn find_exact_duplicates(db: &StateDb) -> Result<Vec<DuplicateSet>> {
    let groups = db.find_duplicate_hashes()?;

    let mut sets = Vec::new();
    for (hash, records) in groups {
        let files: Vec<DuplicateFile> = records
            .into_iter()
            .map(|r| DuplicateFile {
                path: r.file_path,
                size: r.file_size as u64,
                modified: r.file_modified,
            })
            .collect();

        if files.len() > 1 {
            sets.push(DuplicateSet {
                file_hash: hash,
                files,
            });
        }
    }

    Ok(sets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_exact_duplicates_no_duplicates() {
        let db = StateDb::open_in_memory().unwrap();
        db.upsert_file(
            "hash1",
            "/a.jpg",
            100,
            "2024-01-01T00:00:00Z",
            "2024-01-01T00:00:00Z",
        )
        .unwrap();
        db.upsert_file(
            "hash2",
            "/b.jpg",
            200,
            "2024-01-01T00:00:00Z",
            "2024-01-01T00:00:00Z",
        )
        .unwrap();

        let sets = find_exact_duplicates(&db).unwrap();
        assert!(sets.is_empty());
    }

    #[test]
    fn test_find_exact_duplicates_with_duplicates() {
        let db = StateDb::open_in_memory().unwrap();

        // Insert initial files to satisfy foreign key if needed
        db.upsert_file(
            "hash1",
            "/a.jpg",
            100,
            "2024-01-01T00:00:00Z",
            "2024-01-01T00:00:00Z",
        )
        .unwrap();

        // Modify the table to allow duplicate hashes for testing.
        // The standard schema has file_hash as PK, so we drop and recreate.
        let conn = db.raw_connection();
        conn.execute_batch(
            "DROP TABLE IF EXISTS files;
             CREATE TABLE files (
                 id              INTEGER PRIMARY KEY AUTOINCREMENT,
                 file_hash       TEXT NOT NULL,
                 file_path       TEXT NOT NULL,
                 file_size       INTEGER NOT NULL,
                 file_modified   TEXT NOT NULL,
                 first_seen      TEXT NOT NULL,
                 last_seen       TEXT NOT NULL,
                 status          TEXT NOT NULL
             );
             INSERT INTO files (file_hash, file_path, file_size, file_modified, first_seen, last_seen, status)
             VALUES ('hash_dup', '/photos/a.jpg', 1024, '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'active');
             INSERT INTO files (file_hash, file_path, file_size, file_modified, first_seen, last_seen, status)
             VALUES ('hash_dup', '/backup/a.jpg', 1024, '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'active');
             INSERT INTO files (file_hash, file_path, file_size, file_modified, first_seen, last_seen, status)
             VALUES ('hash_dup', '/archive/a.jpg', 1024, '2024-02-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-02-01T00:00:00Z', 'active');
             INSERT INTO files (file_hash, file_path, file_size, file_modified, first_seen, last_seen, status)
             VALUES ('hash_unique', '/photos/b.jpg', 2048, '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'active');
             INSERT INTO files (file_hash, file_path, file_size, file_modified, first_seen, last_seen, status)
             VALUES ('hash_deleted', '/photos/c.jpg', 512, '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'deleted');
             INSERT INTO files (file_hash, file_path, file_size, file_modified, first_seen, last_seen, status)
             VALUES ('hash_deleted', '/photos/d.jpg', 512, '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'deleted');",
        )
        .unwrap();

        let sets = find_exact_duplicates(&db).unwrap();
        assert_eq!(sets.len(), 1, "Should find exactly one duplicate set");
        assert_eq!(sets[0].file_hash, "hash_dup");
        assert_eq!(sets[0].files.len(), 3, "Duplicate set should have 3 files");

        // Verify all paths are present
        let paths: Vec<&str> = sets[0].files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"/photos/a.jpg"));
        assert!(paths.contains(&"/backup/a.jpg"));
        assert!(paths.contains(&"/archive/a.jpg"));
    }

    #[test]
    fn test_duplicate_set_serialization() {
        let set = DuplicateSet {
            file_hash: "abc123".to_string(),
            files: vec![
                DuplicateFile {
                    path: "/a.jpg".to_string(),
                    size: 1024,
                    modified: "2024-01-01".to_string(),
                },
                DuplicateFile {
                    path: "/b.jpg".to_string(),
                    size: 1024,
                    modified: "2024-01-01".to_string(),
                },
            ],
        };

        let json = serde_json::to_string(&set).unwrap();
        assert!(json.contains("abc123"));
        assert!(json.contains("/a.jpg"));
        assert!(json.contains("/b.jpg"));
    }
}
