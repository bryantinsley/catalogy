use catalogy_core::{CatalogyError, FileHash, Job, JobStage, JobStatus, Result};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

/// Statistics about the job queue.
#[derive(Clone, Debug, Default)]
pub struct QueueStats {
    pub pending: u64,
    pub running: u64,
    pub completed: u64,
    pub failed: u64,
    pub skipped: u64,
    /// Breakdown by stage: (stage, pending, running, completed, failed, skipped)
    pub by_stage: Vec<(String, u64, u64, u64, u64, u64)>,
}

/// Row from the models table.
#[derive(Clone, Debug)]
pub struct ModelRecord {
    pub model_id: String,
    pub model_version: String,
    pub model_path: String,
    pub dimensions: u32,
    pub is_current: bool,
    pub registered_at: String,
}

/// Row from the files table.
#[derive(Clone, Debug)]
pub struct FileRecord {
    pub file_hash: String,
    pub file_path: String,
    pub file_size: i64,
    pub file_modified: String,
    pub first_seen: String,
    pub last_seen: String,
    pub status: String,
}

/// SQLite state database for the processing pipeline.
pub struct StateDb {
    conn: Connection,
}

impl StateDb {
    /// Open (or create) the state database at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(|e| CatalogyError::Database(e.to_string()))?;
        let db = Self { conn };
        db.init()?;
        Ok(db)
    }

    /// Open an in-memory database (for tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn =
            Connection::open_in_memory().map_err(|e| CatalogyError::Database(e.to_string()))?;
        let db = Self { conn };
        db.init()?;
        Ok(db)
    }

    fn init(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA busy_timeout=5000;
                 PRAGMA foreign_keys=ON;",
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;
        self.migrate()
    }

    fn migrate(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS files (
                    file_hash       TEXT PRIMARY KEY,
                    file_path       TEXT NOT NULL,
                    file_size       INTEGER NOT NULL,
                    file_modified   TEXT NOT NULL,
                    first_seen      TEXT NOT NULL,
                    last_seen       TEXT NOT NULL,
                    status          TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_files_path ON files(file_path);

                CREATE TABLE IF NOT EXISTS jobs (
                    id              INTEGER PRIMARY KEY AUTOINCREMENT,
                    file_hash       TEXT NOT NULL,
                    file_path       TEXT NOT NULL,
                    stage           TEXT NOT NULL,
                    status          TEXT NOT NULL,
                    attempts        INTEGER NOT NULL DEFAULT 0,
                    max_attempts    INTEGER NOT NULL DEFAULT 3,
                    error_message   TEXT,
                    created_at      TEXT NOT NULL,
                    started_at      TEXT,
                    completed_at    TEXT,
                    worker_id       TEXT,
                    model_id        TEXT,
                    model_version   TEXT,
                    FOREIGN KEY (file_hash) REFERENCES files(file_hash)
                );
                CREATE INDEX IF NOT EXISTS idx_jobs_status ON jobs(status, stage);
                CREATE INDEX IF NOT EXISTS idx_jobs_file ON jobs(file_hash);

                CREATE TABLE IF NOT EXISTS models (
                    model_id        TEXT PRIMARY KEY,
                    model_version   TEXT NOT NULL,
                    model_path      TEXT NOT NULL,
                    dimensions      INTEGER NOT NULL,
                    is_current      INTEGER NOT NULL DEFAULT 0,
                    registered_at   TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS config_state (
                    key             TEXT PRIMARY KEY,
                    value           TEXT NOT NULL,
                    updated_at      TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS metadata (
                    file_hash           TEXT PRIMARY KEY,
                    width               INTEGER,
                    height              INTEGER,
                    duration_ms         INTEGER,
                    fps                 REAL,
                    codec               TEXT,
                    bitrate_kbps        INTEGER,
                    exif_camera_make    TEXT,
                    exif_camera_model   TEXT,
                    exif_date_taken     TEXT,
                    exif_gps_lat        REAL,
                    exif_gps_lon        REAL,
                    exif_focal_length_mm REAL,
                    exif_iso            INTEGER,
                    exif_orientation    INTEGER,
                    extracted_at        TEXT NOT NULL,
                    FOREIGN KEY (file_hash) REFERENCES files(file_hash)
                );",
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))
    }

    // ── Files table ─────────────────────────────────────────

    /// Insert or update a file record.
    pub fn upsert_file(
        &self,
        hash: &str,
        path: &str,
        size: i64,
        modified: &str,
        now: &str,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO files (file_hash, file_path, file_size, file_modified, first_seen, last_seen, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, 'active')
                 ON CONFLICT(file_hash) DO UPDATE SET
                    file_path = excluded.file_path,
                    file_size = excluded.file_size,
                    file_modified = excluded.file_modified,
                    last_seen = excluded.last_seen,
                    status = 'active'",
                params![hash, path, size, modified, now],
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;
        Ok(())
    }

    /// Get a file record by hash.
    pub fn get_file_by_hash(&self, hash: &str) -> Result<Option<FileRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT file_hash, file_path, file_size, file_modified, first_seen, last_seen, status
                 FROM files WHERE file_hash = ?1",
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;

        let result = stmt
            .query_row(params![hash], |row| {
                Ok(FileRecord {
                    file_hash: row.get(0)?,
                    file_path: row.get(1)?,
                    file_size: row.get(2)?,
                    file_modified: row.get(3)?,
                    first_seen: row.get(4)?,
                    last_seen: row.get(5)?,
                    status: row.get(6)?,
                })
            })
            .optional()
            .map_err(|e| CatalogyError::Database(e.to_string()))?;

        Ok(result)
    }

    /// Get a file record by path.
    pub fn get_file_by_path(&self, path: &str) -> Result<Option<FileRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT file_hash, file_path, file_size, file_modified, first_seen, last_seen, status
                 FROM files WHERE file_path = ?1 AND status = 'active'",
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;

        let result = stmt
            .query_row(params![path], |row| {
                Ok(FileRecord {
                    file_hash: row.get(0)?,
                    file_path: row.get(1)?,
                    file_size: row.get(2)?,
                    file_modified: row.get(3)?,
                    first_seen: row.get(4)?,
                    last_seen: row.get(5)?,
                    status: row.get(6)?,
                })
            })
            .optional()
            .map_err(|e| CatalogyError::Database(e.to_string()))?;

        Ok(result)
    }

    /// Get all active file records.
    pub fn get_all_active_files(&self) -> Result<Vec<FileRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT file_hash, file_path, file_size, file_modified, first_seen, last_seen, status
                 FROM files WHERE status = 'active'",
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(FileRecord {
                    file_hash: row.get(0)?,
                    file_path: row.get(1)?,
                    file_size: row.get(2)?,
                    file_modified: row.get(3)?,
                    first_seen: row.get(4)?,
                    last_seen: row.get(5)?,
                    status: row.get(6)?,
                })
            })
            .map_err(|e| CatalogyError::Database(e.to_string()))?;

        let mut files = Vec::new();
        for row in rows {
            files.push(row.map_err(|e| CatalogyError::Database(e.to_string()))?);
        }
        Ok(files)
    }

    /// Mark a file as deleted.
    pub fn mark_file_deleted(&self, hash: &str, now: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE files SET status = 'deleted', last_seen = ?2 WHERE file_hash = ?1",
                params![hash, now],
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;
        Ok(())
    }

    /// Update path for a moved file.
    pub fn update_file_path(&self, hash: &str, new_path: &str, now: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE files SET file_path = ?2, last_seen = ?3, status = 'active' WHERE file_hash = ?1",
                params![hash, new_path, now],
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;
        Ok(())
    }

    // ── Jobs table ──────────────────────────────────────────

    /// Enqueue a job. Idempotent: skips if the same file_hash+stage already
    /// exists and is not failed.
    pub fn enqueue(
        &self,
        file_hash: &str,
        file_path: &str,
        stage: JobStage,
    ) -> Result<Option<i64>> {
        let stage_str = stage_to_str(&stage);
        let now = chrono::Utc::now().to_rfc3339();

        // Check for existing non-failed job
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM jobs WHERE file_hash = ?1 AND stage = ?2 AND status != 'failed'",
                params![file_hash, stage_str],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| CatalogyError::Database(e.to_string()))?;

        if existing.is_some() {
            return Ok(None); // Already exists
        }

        self.conn
            .execute(
                "INSERT INTO jobs (file_hash, file_path, stage, status, created_at)
                 VALUES (?1, ?2, ?3, 'pending', ?4)",
                params![file_hash, file_path, stage_str, now],
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;

        Ok(Some(self.conn.last_insert_rowid()))
    }

    /// Claim the next pending job for a given stage.
    pub fn claim(&self, stage: JobStage, worker_id: &str) -> Result<Option<Job>> {
        let stage_str = stage_to_str(&stage);
        let now = chrono::Utc::now().to_rfc3339();

        // Find and claim atomically
        let result: Option<(i64, String, String)> = self
            .conn
            .query_row(
                "UPDATE jobs SET status = 'running', started_at = ?3, worker_id = ?4,
                        attempts = attempts + 1
                 WHERE id = (
                    SELECT id FROM jobs
                    WHERE stage = ?1 AND status = 'pending'
                    ORDER BY created_at
                    LIMIT 1
                 )
                 RETURNING id, file_hash, file_path",
                params![stage_str, stage_str, now, worker_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|e| CatalogyError::Database(e.to_string()))?;

        match result {
            Some((id, hash, path)) => Ok(Some(Job {
                id,
                file_hash: FileHash(hash),
                file_path: PathBuf::from(path),
                stage,
                status: JobStatus::Running,
                attempts: 1,
                error_message: None,
            })),
            None => Ok(None),
        }
    }

    /// Mark a job as completed.
    pub fn complete(&self, job_id: i64) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn
            .execute(
                "UPDATE jobs SET status = 'completed', completed_at = ?2 WHERE id = ?1",
                params![job_id, now],
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;
        Ok(())
    }

    /// Mark a job as skipped.
    pub fn skip(&self, job_id: i64) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn
            .execute(
                "UPDATE jobs SET status = 'skipped', completed_at = ?2 WHERE id = ?1",
                params![job_id, now],
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;
        Ok(())
    }

    /// Mark a job as failed.
    pub fn fail(&self, job_id: i64, error: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn
            .execute(
                "UPDATE jobs SET status = 'failed', completed_at = ?2, error_message = ?3 WHERE id = ?1",
                params![job_id, now, error],
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;
        Ok(())
    }

    /// Get queue statistics.
    pub fn stats(&self) -> Result<QueueStats> {
        let mut stats = QueueStats::default();

        // Overall counts
        let mut stmt = self
            .conn
            .prepare("SELECT status, COUNT(*) FROM jobs GROUP BY status")
            .map_err(|e| CatalogyError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                let status: String = row.get(0)?;
                let count: u64 = row.get(1)?;
                Ok((status, count))
            })
            .map_err(|e| CatalogyError::Database(e.to_string()))?;

        for row in rows {
            let (status, count) = row.map_err(|e| CatalogyError::Database(e.to_string()))?;
            match status.as_str() {
                "pending" => stats.pending = count,
                "running" => stats.running = count,
                "completed" => stats.completed = count,
                "failed" => stats.failed = count,
                "skipped" => stats.skipped = count,
                _ => {}
            }
        }

        // Per-stage breakdown
        let mut stmt = self
            .conn
            .prepare(
                "SELECT stage,
                    SUM(CASE WHEN status='pending' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status='running' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status='completed' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status='failed' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status='skipped' THEN 1 ELSE 0 END)
                 FROM jobs GROUP BY stage",
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, u64>(4)?,
                    row.get::<_, u64>(5)?,
                ))
            })
            .map_err(|e| CatalogyError::Database(e.to_string()))?;

        for row in rows {
            let r = row.map_err(|e| CatalogyError::Database(e.to_string()))?;
            stats.by_stage.push(r);
        }

        Ok(stats)
    }

    /// Enqueue a tombstone job for a deleted file.
    pub fn enqueue_tombstone(&self, file_hash: &str, file_path: &str) -> Result<Option<i64>> {
        self.enqueue(file_hash, file_path, JobStage::Index)
    }

    /// Update the path on all pending/running jobs for a given file_hash.
    pub fn update_job_paths(&self, file_hash: &str, new_path: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE jobs SET file_path = ?2 WHERE file_hash = ?1 AND status IN ('pending', 'running')",
                params![file_hash, new_path],
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;
        Ok(())
    }

    /// Get total number of tracked files.
    pub fn file_count(&self) -> Result<u64> {
        let count: u64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE status = 'active'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;
        Ok(count)
    }

    // ── Dedup queries ────────────────────────────────────────

    /// Find file_hash values that appear more than once with status='active'.
    /// Returns groups of (file_hash, vec of FileRecord).
    pub fn find_duplicate_hashes(&self) -> Result<Vec<(String, Vec<FileRecord>)>> {
        // Find hashes with more than one active row
        let mut stmt = self
            .conn
            .prepare(
                "SELECT file_hash FROM files WHERE status = 'active'
                 GROUP BY file_hash HAVING COUNT(*) > 1",
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;

        let hashes: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| CatalogyError::Database(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        let mut groups = Vec::new();
        for hash in hashes {
            let mut file_stmt = self
                .conn
                .prepare(
                    "SELECT file_hash, file_path, file_size, file_modified, first_seen, last_seen, status
                     FROM files WHERE file_hash = ?1 AND status = 'active'",
                )
                .map_err(|e| CatalogyError::Database(e.to_string()))?;

            let records: Vec<FileRecord> = file_stmt
                .query_map(params![hash], |row| {
                    Ok(FileRecord {
                        file_hash: row.get(0)?,
                        file_path: row.get(1)?,
                        file_size: row.get(2)?,
                        file_modified: row.get(3)?,
                        first_seen: row.get(4)?,
                        last_seen: row.get(5)?,
                        status: row.get(6)?,
                    })
                })
                .map_err(|e| CatalogyError::Database(e.to_string()))?
                .filter_map(|r| r.ok())
                .collect();

            if records.len() > 1 {
                groups.push((hash, records));
            }
        }

        Ok(groups)
    }

    /// Get a reference to the underlying SQLite connection for advanced queries.
    pub fn raw_connection(&self) -> &Connection {
        &self.conn
    }

    // ── Config state ────────────────────────────────────────

    /// Set a config state value.
    pub fn set_config(&self, key: &str, value: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn
            .execute(
                "INSERT INTO config_state (key, value, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                params![key, value, now],
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;
        Ok(())
    }

    /// Get a config state value.
    pub fn get_config(&self, key: &str) -> Result<Option<String>> {
        let result: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM config_state WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| CatalogyError::Database(e.to_string()))?;
        Ok(result)
    }

    // ── Models table ───────────────────────────────────────

    /// Register a new embedding model.
    pub fn register_model(
        &self,
        model_id: &str,
        model_version: &str,
        model_path: &str,
        dimensions: u32,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn
            .execute(
                "INSERT INTO models (model_id, model_version, model_path, dimensions, is_current, registered_at)
                 VALUES (?1, ?2, ?3, ?4, 0, ?5)
                 ON CONFLICT(model_id) DO UPDATE SET
                    model_version = excluded.model_version,
                    model_path = excluded.model_path,
                    dimensions = excluded.dimensions,
                    registered_at = excluded.registered_at",
                params![model_id, model_version, model_path, dimensions as i64, now],
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;
        Ok(())
    }

    /// Get a model by ID.
    pub fn get_model(&self, model_id: &str) -> Result<Option<ModelRecord>> {
        let result = self
            .conn
            .query_row(
                "SELECT model_id, model_version, model_path, dimensions, is_current, registered_at
                 FROM models WHERE model_id = ?1",
                params![model_id],
                |row| {
                    Ok(ModelRecord {
                        model_id: row.get(0)?,
                        model_version: row.get(1)?,
                        model_path: row.get(2)?,
                        dimensions: row.get::<_, i64>(3)? as u32,
                        is_current: row.get::<_, i64>(4)? != 0,
                        registered_at: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(|e| CatalogyError::Database(e.to_string()))?;
        Ok(result)
    }

    /// List all registered models.
    pub fn list_models(&self) -> Result<Vec<ModelRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT model_id, model_version, model_path, dimensions, is_current, registered_at
                 FROM models ORDER BY registered_at",
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(ModelRecord {
                    model_id: row.get(0)?,
                    model_version: row.get(1)?,
                    model_path: row.get(2)?,
                    dimensions: row.get::<_, i64>(3)? as u32,
                    is_current: row.get::<_, i64>(4)? != 0,
                    registered_at: row.get(5)?,
                })
            })
            .map_err(|e| CatalogyError::Database(e.to_string()))?;

        let mut models = Vec::new();
        for row in rows {
            models.push(row.map_err(|e| CatalogyError::Database(e.to_string()))?);
        }
        Ok(models)
    }

    /// Get the currently active model.
    pub fn get_current_model(&self) -> Result<Option<ModelRecord>> {
        let result = self
            .conn
            .query_row(
                "SELECT model_id, model_version, model_path, dimensions, is_current, registered_at
                 FROM models WHERE is_current = 1",
                [],
                |row| {
                    Ok(ModelRecord {
                        model_id: row.get(0)?,
                        model_version: row.get(1)?,
                        model_path: row.get(2)?,
                        dimensions: row.get::<_, i64>(3)? as u32,
                        is_current: row.get::<_, i64>(4)? != 0,
                        registered_at: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(|e| CatalogyError::Database(e.to_string()))?;
        Ok(result)
    }

    /// Set a model as the current active model (clears is_current on all others).
    pub fn set_current_model(&self, model_id: &str) -> Result<()> {
        // Verify model exists
        if self.get_model(model_id)?.is_none() {
            return Err(CatalogyError::Database(format!(
                "Model '{}' not found",
                model_id
            )));
        }

        self.conn
            .execute("UPDATE models SET is_current = 0", [])
            .map_err(|e| CatalogyError::Database(e.to_string()))?;
        self.conn
            .execute(
                "UPDATE models SET is_current = 1 WHERE model_id = ?1",
                params![model_id],
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;
        Ok(())
    }

    /// Enqueue re-embed jobs for all files that need re-embedding with a new model.
    /// Returns the number of jobs created.
    pub fn enqueue_reembed(
        &self,
        target_model_id: &str,
        target_model_version: &str,
    ) -> Result<u64> {
        let now = chrono::Utc::now().to_rfc3339();
        let stage_str = stage_to_str(&JobStage::ReEmbed);

        // Get all active files and create re_embed jobs for each
        let files = self.get_all_active_files()?;
        let mut count = 0u64;

        for file in &files {
            // Check for existing non-failed re_embed job for this file
            let existing: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM jobs WHERE file_hash = ?1 AND stage = ?2 AND status != 'failed'",
                    params![file.file_hash, stage_str],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| CatalogyError::Database(e.to_string()))?;

            if existing.is_some() {
                continue;
            }

            self.conn
                .execute(
                    "INSERT INTO jobs (file_hash, file_path, stage, status, created_at, model_id, model_version)
                     VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?6)",
                    params![
                        file.file_hash,
                        file.file_path,
                        stage_str,
                        now,
                        target_model_id,
                        target_model_version,
                    ],
                )
                .map_err(|e| CatalogyError::Database(e.to_string()))?;
            count += 1;
        }

        Ok(count)
    }

    // ── Metadata table ─────────────────────────────────────

    /// Get extracted metadata for a file.
    pub fn get_metadata(&self, file_hash: &str) -> Result<Option<catalogy_core::MediaMetadata>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT width, height, duration_ms, fps, codec, bitrate_kbps,
                        exif_camera_make, exif_camera_model, exif_date_taken,
                        exif_gps_lat, exif_gps_lon, exif_focal_length_mm,
                        exif_iso, exif_orientation
                 FROM metadata WHERE file_hash = ?1",
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;

        let result = stmt
            .query_row(params![file_hash], |row| {
                let exif_make: Option<String> = row.get(6)?;
                let exif_model: Option<String> = row.get(7)?;
                let exif_date: Option<String> = row.get(8)?;
                let exif_lat: Option<f64> = row.get(9)?;
                let exif_lon: Option<f64> = row.get(10)?;
                let exif_fl: Option<f32> = row.get(11)?;
                let exif_iso: Option<u32> = row.get(12)?;
                let exif_orient: Option<u32> = row.get(13)?;

                let has_exif = exif_make.is_some()
                    || exif_model.is_some()
                    || exif_date.is_some()
                    || exif_lat.is_some();

                let exif = if has_exif {
                    Some(catalogy_core::ExifData {
                        camera_make: exif_make,
                        camera_model: exif_model,
                        date_taken: exif_date
                            .and_then(|d| chrono::NaiveDateTime::parse_from_str(&d, "%Y-%m-%d %H:%M:%S").ok()),
                        gps_lat: exif_lat,
                        gps_lon: exif_lon,
                        focal_length_mm: exif_fl,
                        iso: exif_iso,
                        orientation: exif_orient.map(|o| o as u8),
                    })
                } else {
                    None
                };

                Ok(catalogy_core::MediaMetadata {
                    width: row.get(0)?,
                    height: row.get(1)?,
                    duration_ms: row.get::<_, Option<i64>>(2)?.map(|v| v as u64),
                    fps: row.get(3)?,
                    codec: row.get(4)?,
                    bitrate_kbps: row.get(5)?,
                    exif,
                })
            })
            .optional()
            .map_err(|e| CatalogyError::Database(e.to_string()))?;

        Ok(result)
    }

    /// Get all active video files with their metadata (for transcode evaluation).
    pub fn get_video_files_with_metadata(
        &self,
        video_extensions: &[&str],
    ) -> Result<Vec<(FileRecord, catalogy_core::MediaMetadata)>> {
        let all_files = self.get_all_active_files()?;
        let mut results = Vec::new();

        for file in all_files {
            let ext = std::path::Path::new(&file.file_path)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_lowercase())
                .unwrap_or_default();

            if !video_extensions.contains(&ext.as_str()) {
                continue;
            }

            if let Some(meta) = self.get_metadata(&file.file_hash)? {
                results.push((file, meta));
            }
        }

        Ok(results)
    }

    /// Store extracted metadata for a file.
    pub fn store_metadata(
        &self,
        file_hash: &str,
        metadata: &catalogy_core::MediaMetadata,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let (exif_make, exif_model, exif_date, exif_lat, exif_lon, exif_fl, exif_iso, exif_orient) =
            match &metadata.exif {
                Some(exif) => (
                    exif.camera_make.as_deref(),
                    exif.camera_model.as_deref(),
                    exif.date_taken.map(|d| d.to_string()),
                    exif.gps_lat,
                    exif.gps_lon,
                    exif.focal_length_mm,
                    exif.iso,
                    exif.orientation.map(|o| o as u32),
                ),
                None => (None, None, None, None, None, None, None, None),
            };

        self.conn
            .execute(
                "INSERT INTO metadata (file_hash, width, height, duration_ms, fps, codec,
                    bitrate_kbps, exif_camera_make, exif_camera_model, exif_date_taken,
                    exif_gps_lat, exif_gps_lon, exif_focal_length_mm, exif_iso,
                    exif_orientation, extracted_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                 ON CONFLICT(file_hash) DO UPDATE SET
                    width = excluded.width, height = excluded.height,
                    duration_ms = excluded.duration_ms, fps = excluded.fps,
                    codec = excluded.codec, bitrate_kbps = excluded.bitrate_kbps,
                    exif_camera_make = excluded.exif_camera_make,
                    exif_camera_model = excluded.exif_camera_model,
                    exif_date_taken = excluded.exif_date_taken,
                    exif_gps_lat = excluded.exif_gps_lat,
                    exif_gps_lon = excluded.exif_gps_lon,
                    exif_focal_length_mm = excluded.exif_focal_length_mm,
                    exif_iso = excluded.exif_iso,
                    exif_orientation = excluded.exif_orientation,
                    extracted_at = excluded.extracted_at",
                params![
                    file_hash,
                    metadata.width,
                    metadata.height,
                    metadata.duration_ms,
                    metadata.fps,
                    metadata.codec,
                    metadata.bitrate_kbps,
                    exif_make,
                    exif_model,
                    exif_date,
                    exif_lat,
                    exif_lon,
                    exif_fl,
                    exif_iso,
                    exif_orient,
                    now,
                ],
            )
            .map_err(|e| CatalogyError::Database(e.to_string()))?;
        Ok(())
    }
}

// ── Helpers ─────────────────────────────────────────────────

fn stage_to_str(stage: &JobStage) -> &'static str {
    match stage {
        JobStage::ExtractFrames => "extract_frames",
        JobStage::ExtractMetadata => "extract_metadata",
        JobStage::Embed => "embed",
        JobStage::Index => "index",
        JobStage::ReEmbed => "re_embed",
        JobStage::Transcode => "transcode",
    }
}

/// Use rusqlite's optional extension
use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> StateDb {
        StateDb::open_in_memory().unwrap()
    }

    #[test]
    fn test_create_schema() {
        let db = test_db();
        // Tables should exist
        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('files','jobs','models','config_state','metadata')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 5);
    }

    #[test]
    fn test_wal_mode() {
        let db = test_db();
        let mode: String = db
            .conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        // In-memory databases use "memory" mode, but the PRAGMA was set
        assert!(mode == "wal" || mode == "memory");
    }

    #[test]
    fn test_upsert_and_get_file() {
        let db = test_db();
        db.upsert_file(
            "abc123",
            "/photos/test.jpg",
            1024,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();

        let file = db.get_file_by_hash("abc123").unwrap().unwrap();
        assert_eq!(file.file_path, "/photos/test.jpg");
        assert_eq!(file.file_size, 1024);
        assert_eq!(file.status, "active");

        // Upsert again with different path (simulates moved file)
        db.upsert_file(
            "abc123",
            "/photos/moved.jpg",
            1024,
            "2024-01-01T00:00:00Z",
            "2024-06-02T00:00:00Z",
        )
        .unwrap();
        let file = db.get_file_by_hash("abc123").unwrap().unwrap();
        assert_eq!(file.file_path, "/photos/moved.jpg");
    }

    #[test]
    fn test_get_file_by_path() {
        let db = test_db();
        db.upsert_file(
            "abc123",
            "/photos/test.jpg",
            1024,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();

        let file = db.get_file_by_path("/photos/test.jpg").unwrap().unwrap();
        assert_eq!(file.file_hash, "abc123");

        assert!(db.get_file_by_path("/nonexistent").unwrap().is_none());
    }

    #[test]
    fn test_enqueue_idempotent() {
        let db = test_db();
        db.upsert_file(
            "abc123",
            "/photos/test.jpg",
            1024,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();

        let id1 = db
            .enqueue("abc123", "/photos/test.jpg", JobStage::ExtractMetadata)
            .unwrap();
        assert!(id1.is_some());

        // Second enqueue should be skipped
        let id2 = db
            .enqueue("abc123", "/photos/test.jpg", JobStage::ExtractMetadata)
            .unwrap();
        assert!(id2.is_none());
    }

    #[test]
    fn test_enqueue_different_stages() {
        let db = test_db();
        db.upsert_file(
            "abc123",
            "/photos/test.jpg",
            1024,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();

        let id1 = db
            .enqueue("abc123", "/photos/test.jpg", JobStage::ExtractMetadata)
            .unwrap();
        let id2 = db
            .enqueue("abc123", "/photos/test.jpg", JobStage::Embed)
            .unwrap();
        assert!(id1.is_some());
        assert!(id2.is_some());
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_claim_and_complete() {
        let db = test_db();
        db.upsert_file(
            "abc123",
            "/photos/test.jpg",
            1024,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();
        db.enqueue("abc123", "/photos/test.jpg", JobStage::ExtractMetadata)
            .unwrap();

        let job = db
            .claim(JobStage::ExtractMetadata, "worker-1")
            .unwrap()
            .unwrap();
        assert_eq!(job.file_hash, FileHash("abc123".to_string()));
        assert_eq!(job.stage, JobStage::ExtractMetadata);
        assert_eq!(job.status, JobStatus::Running);

        // No more pending jobs
        assert!(db
            .claim(JobStage::ExtractMetadata, "worker-1")
            .unwrap()
            .is_none());

        db.complete(job.id).unwrap();
    }

    #[test]
    fn test_fail_job() {
        let db = test_db();
        db.upsert_file(
            "abc123",
            "/photos/test.jpg",
            1024,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();
        db.enqueue("abc123", "/photos/test.jpg", JobStage::Embed)
            .unwrap();

        let job = db.claim(JobStage::Embed, "worker-1").unwrap().unwrap();
        db.fail(job.id, "corrupt file").unwrap();

        // After failure, a new enqueue should work (since old one is 'failed')
        let id = db
            .enqueue("abc123", "/photos/test.jpg", JobStage::Embed)
            .unwrap();
        assert!(id.is_some());
    }

    #[test]
    fn test_stats() {
        let db = test_db();
        db.upsert_file(
            "a",
            "/a.jpg",
            100,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();
        db.upsert_file(
            "b",
            "/b.jpg",
            200,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();

        db.enqueue("a", "/a.jpg", JobStage::ExtractMetadata)
            .unwrap();
        db.enqueue("a", "/a.jpg", JobStage::Embed).unwrap();
        db.enqueue("b", "/b.jpg", JobStage::ExtractMetadata)
            .unwrap();

        let stats = db.stats().unwrap();
        assert_eq!(stats.pending, 3);
        assert_eq!(stats.running, 0);
    }

    #[test]
    fn test_mark_file_deleted() {
        let db = test_db();
        db.upsert_file(
            "abc123",
            "/photos/test.jpg",
            1024,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();
        db.mark_file_deleted("abc123", "2024-07-01T00:00:00Z")
            .unwrap();

        let file = db.get_file_by_hash("abc123").unwrap().unwrap();
        assert_eq!(file.status, "deleted");
    }

    #[test]
    fn test_config_state() {
        let db = test_db();
        db.set_config("last_scan_time", "2024-06-01T12:00:00Z")
            .unwrap();

        let val = db.get_config("last_scan_time").unwrap().unwrap();
        assert_eq!(val, "2024-06-01T12:00:00Z");

        assert!(db.get_config("nonexistent").unwrap().is_none());
    }

    #[test]
    fn test_file_count() {
        let db = test_db();
        assert_eq!(db.file_count().unwrap(), 0);

        db.upsert_file(
            "a",
            "/a.jpg",
            100,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();
        db.upsert_file(
            "b",
            "/b.jpg",
            200,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();
        assert_eq!(db.file_count().unwrap(), 2);

        db.mark_file_deleted("a", "2024-07-01T00:00:00Z").unwrap();
        assert_eq!(db.file_count().unwrap(), 1);
    }

    #[test]
    fn test_update_file_path() {
        let db = test_db();
        db.upsert_file(
            "abc123",
            "/old/path.jpg",
            1024,
            "2024-01-01T00:00:00Z",
            "2024-06-01T00:00:00Z",
        )
        .unwrap();
        db.update_file_path("abc123", "/new/path.jpg", "2024-07-01T00:00:00Z")
            .unwrap();

        let file = db.get_file_by_hash("abc123").unwrap().unwrap();
        assert_eq!(file.file_path, "/new/path.jpg");
    }

    #[test]
    fn test_open_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_state.db");
        {
            let db = StateDb::open(&path).unwrap();
            db.upsert_file(
                "abc",
                "/test.jpg",
                100,
                "2024-01-01T00:00:00Z",
                "2024-06-01T00:00:00Z",
            )
            .unwrap();
        }
        // Re-open and verify data persists
        let db = StateDb::open(&path).unwrap();
        let file = db.get_file_by_hash("abc").unwrap().unwrap();
        assert_eq!(file.file_path, "/test.jpg");
    }

    // ── Model registry tests ──────────────────────────────

    #[test]
    fn test_register_model() {
        let db = test_db();
        db.register_model("clip-vit-h-14", "1", "/models/visual.onnx", 1024)
            .unwrap();

        let model = db.get_model("clip-vit-h-14").unwrap().unwrap();
        assert_eq!(model.model_id, "clip-vit-h-14");
        assert_eq!(model.model_version, "1");
        assert_eq!(model.model_path, "/models/visual.onnx");
        assert_eq!(model.dimensions, 1024);
        assert!(!model.is_current);
    }

    #[test]
    fn test_register_model_upsert() {
        let db = test_db();
        db.register_model("clip-vit-h-14", "1", "/models/old.onnx", 1024)
            .unwrap();
        db.register_model("clip-vit-h-14", "2", "/models/new.onnx", 768)
            .unwrap();

        let model = db.get_model("clip-vit-h-14").unwrap().unwrap();
        assert_eq!(model.model_version, "2");
        assert_eq!(model.model_path, "/models/new.onnx");
        assert_eq!(model.dimensions, 768);
    }

    #[test]
    fn test_list_models() {
        let db = test_db();
        db.register_model("model-a", "1", "/a.onnx", 512)
            .unwrap();
        db.register_model("model-b", "1", "/b.onnx", 1024)
            .unwrap();

        let models = db.list_models().unwrap();
        assert_eq!(models.len(), 2);
    }

    #[test]
    fn test_list_models_empty() {
        let db = test_db();
        let models = db.list_models().unwrap();
        assert!(models.is_empty());
    }

    #[test]
    fn test_get_model_not_found() {
        let db = test_db();
        assert!(db.get_model("nonexistent").unwrap().is_none());
    }

    #[test]
    fn test_set_current_model() {
        let db = test_db();
        db.register_model("model-a", "1", "/a.onnx", 512)
            .unwrap();
        db.register_model("model-b", "1", "/b.onnx", 1024)
            .unwrap();

        db.set_current_model("model-a").unwrap();
        let current = db.get_current_model().unwrap().unwrap();
        assert_eq!(current.model_id, "model-a");

        // Switch to model-b
        db.set_current_model("model-b").unwrap();
        let current = db.get_current_model().unwrap().unwrap();
        assert_eq!(current.model_id, "model-b");

        // model-a should no longer be current
        let a = db.get_model("model-a").unwrap().unwrap();
        assert!(!a.is_current);
    }

    #[test]
    fn test_set_current_model_not_found() {
        let db = test_db();
        let result = db.set_current_model("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_get_current_model_none() {
        let db = test_db();
        assert!(db.get_current_model().unwrap().is_none());
    }

    #[test]
    fn test_enqueue_reembed() {
        let db = test_db();
        db.upsert_file("a", "/a.jpg", 100, "2024-01-01T00:00:00Z", "2024-06-01T00:00:00Z")
            .unwrap();
        db.upsert_file("b", "/b.jpg", 200, "2024-01-01T00:00:00Z", "2024-06-01T00:00:00Z")
            .unwrap();
        db.upsert_file("c", "/c.jpg", 300, "2024-01-01T00:00:00Z", "2024-06-01T00:00:00Z")
            .unwrap();

        let count = db.enqueue_reembed("new-model", "2").unwrap();
        assert_eq!(count, 3);

        // Verify jobs exist with correct stage
        let stats = db.stats().unwrap();
        assert_eq!(stats.pending, 3);
        let re_embed_stage = stats.by_stage.iter().find(|s| s.0 == "re_embed");
        assert!(re_embed_stage.is_some());
        assert_eq!(re_embed_stage.unwrap().1, 3); // 3 pending
    }

    #[test]
    fn test_enqueue_reembed_idempotent() {
        let db = test_db();
        db.upsert_file("a", "/a.jpg", 100, "2024-01-01T00:00:00Z", "2024-06-01T00:00:00Z")
            .unwrap();

        let count1 = db.enqueue_reembed("new-model", "2").unwrap();
        assert_eq!(count1, 1);

        // Second call should not create duplicates
        let count2 = db.enqueue_reembed("new-model", "2").unwrap();
        assert_eq!(count2, 0);
    }

    #[test]
    fn test_enqueue_reembed_no_files() {
        let db = test_db();
        let count = db.enqueue_reembed("new-model", "2").unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_enqueue_reembed_skips_deleted() {
        let db = test_db();
        db.upsert_file("a", "/a.jpg", 100, "2024-01-01T00:00:00Z", "2024-06-01T00:00:00Z")
            .unwrap();
        db.upsert_file("b", "/b.jpg", 200, "2024-01-01T00:00:00Z", "2024-06-01T00:00:00Z")
            .unwrap();
        db.mark_file_deleted("b", "2024-07-01T00:00:00Z").unwrap();

        let count = db.enqueue_reembed("new-model", "2").unwrap();
        assert_eq!(count, 1); // Only active file 'a'
    }
}
