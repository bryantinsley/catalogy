# Catalogy — Low-Level Design

## 1. Data Schemas

### 1.1 LanceDB Catalog Table: `media`

The primary catalog table. One row per media item (images get one row; videos get one row per extracted frame plus one row for the video itself with a representative frame).

```
Column                  Type          Nullable  Description
──────────────────────────────────────────────────────────────────────
id                      String        No        UUID v7 (time-sortable)
file_hash               String        No        SHA256 of source file
file_path               String        No        Absolute path to source file
file_name               String        No        Filename without directory
file_size               Int64         No        Size in bytes
file_ext                String        No        Lowercase extension without dot
file_created            Timestamp     Yes       fs creation time (if available)
file_modified           Timestamp     No        fs last-modified time

media_type              String        No        "image" | "video" | "video_frame"
source_video_path       String        Yes       For video_frame: path to source video
frame_index             Int32         Yes       For video_frame: 0-based frame number
frame_timestamp_ms      Int64         Yes       For video_frame: position in video (ms)

width                   Int32         Yes       Pixel width
height                  Int32         Yes       Pixel height
duration_ms             Int64         Yes       Video duration in milliseconds
fps                     Float32       Yes       Video frame rate
codec                   String        Yes       Video/image codec name
bitrate_kbps            Int32         Yes       Video bitrate

exif_camera_make        String        Yes       EXIF: camera manufacturer
exif_camera_model       String        Yes       EXIF: camera model
exif_date_taken         Timestamp     Yes       EXIF: original date/time
exif_gps_lat            Float64       Yes       EXIF: GPS latitude
exif_gps_lon            Float64       Yes       EXIF: GPS longitude
exif_focal_length_mm    Float32       Yes       EXIF: focal length
exif_iso                Int32         Yes       EXIF: ISO speed
exif_orientation        Int32         Yes       EXIF: orientation (1-8)

embedding               FixedSizeList No        CLIP vector (f32 × 1024)
                        [Float32; 1024]
model_id                String        No        e.g. "clip-vit-h-14"
model_version           String        No        e.g. "1" (tracks re-embeds)

indexed_at              Timestamp     No        When this row was written
updated_at              Timestamp     No        Last update time
tombstone               Boolean       No        Soft-delete flag (default false)
```

**Indexes**:
- IVF-PQ on `embedding` (built after bulk ingest, nprobes=20, num_partitions=256 for 1M rows)
- Scalar index on `file_hash` (for dedup lookups)
- Scalar index on `media_type` (for filtered search)
- Scalar index on `file_modified` (for time-range queries)

### 1.2 SQLite State Database: `state.db`

#### Table: `files`

Tracks every known file in the library. Source of truth for change detection.

```sql
CREATE TABLE files (
    file_hash       TEXT PRIMARY KEY,
    file_path       TEXT NOT NULL,
    file_size       INTEGER NOT NULL,
    file_modified   TEXT NOT NULL,       -- ISO 8601
    first_seen      TEXT NOT NULL,       -- ISO 8601
    last_seen       TEXT NOT NULL,       -- ISO 8601
    status          TEXT NOT NULL        -- 'active', 'moved', 'deleted'
);
CREATE INDEX idx_files_path ON files(file_path);
```

#### Table: `jobs`

Processing pipeline state.

```sql
CREATE TABLE jobs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    file_hash       TEXT NOT NULL,
    file_path       TEXT NOT NULL,
    stage           TEXT NOT NULL,       -- 'extract_frames', 'extract_metadata',
                                         -- 'embed', 'index', 're_embed'
    status          TEXT NOT NULL,       -- 'pending', 'running', 'completed',
                                         -- 'failed', 'skipped'
    attempts        INTEGER NOT NULL DEFAULT 0,
    max_attempts    INTEGER NOT NULL DEFAULT 3,
    error_message   TEXT,
    created_at      TEXT NOT NULL,
    started_at      TEXT,
    completed_at    TEXT,
    worker_id       TEXT,               -- for concurrency tracking
    model_id        TEXT,               -- for re_embed jobs
    model_version   TEXT,               -- for re_embed jobs

    FOREIGN KEY (file_hash) REFERENCES files(file_hash)
);
CREATE INDEX idx_jobs_status ON jobs(status, stage);
CREATE INDEX idx_jobs_file ON jobs(file_hash);
```

#### Table: `models`

Registry of embedding models.

```sql
CREATE TABLE models (
    model_id        TEXT PRIMARY KEY,
    model_version   TEXT NOT NULL,
    model_path      TEXT NOT NULL,
    dimensions      INTEGER NOT NULL,
    is_current      INTEGER NOT NULL DEFAULT 0,  -- boolean
    registered_at   TEXT NOT NULL
);
```

#### Table: `config_state`

Runtime state that persists across runs.

```sql
CREATE TABLE config_state (
    key             TEXT PRIMARY KEY,
    value           TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);
-- Stores: last_scan_time, catalog_version, schema_version, etc.
```

## 2. Core Types (catalogy-core)

```rust
// --- Identity ---

/// Time-sortable unique ID for catalog entries
pub type MediaId = uuid::Uuid; // v7

/// Content-based identity for files
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileHash(pub String); // hex-encoded SHA256

// --- Media ---

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaType {
    Image,
    Video,
    VideoFrame,
}

#[derive(Clone, Debug)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub hash: FileHash,
    pub size: u64,
    pub modified: SystemTime,
    pub media_type: MediaType,
}

#[derive(Clone, Debug)]
pub struct MediaMetadata {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub fps: Option<f32>,
    pub codec: Option<String>,
    pub bitrate_kbps: Option<u32>,
    pub exif: Option<ExifData>,
}

#[derive(Clone, Debug)]
pub struct ExifData {
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub date_taken: Option<chrono::NaiveDateTime>,
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
    pub focal_length_mm: Option<f32>,
    pub iso: Option<u32>,
    pub orientation: Option<u8>,
}

#[derive(Clone, Debug)]
pub struct Embedding {
    pub vector: Vec<f32>,
    pub model_id: String,
    pub model_version: String,
}

#[derive(Clone, Debug)]
pub struct ExtractedFrame {
    pub source_video: PathBuf,
    pub frame_index: u32,
    pub timestamp_ms: u64,
    pub image_data: Vec<u8>,  // raw RGB pixels or encoded JPEG
}

// --- Jobs ---

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobStage {
    ExtractFrames,
    ExtractMetadata,
    Embed,
    Index,
    ReEmbed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

#[derive(Clone, Debug)]
pub struct Job {
    pub id: i64,
    pub file_hash: FileHash,
    pub file_path: PathBuf,
    pub stage: JobStage,
    pub status: JobStatus,
    pub attempts: u32,
    pub error_message: Option<String>,
}

// --- Search ---

#[derive(Clone, Debug)]
pub struct SearchQuery {
    pub text: String,
    pub filters: SearchFilters,
    pub limit: usize,
}

#[derive(Clone, Debug, Default)]
pub struct SearchFilters {
    pub media_type: Option<MediaType>,
    pub after: Option<chrono::NaiveDateTime>,
    pub before: Option<chrono::NaiveDateTime>,
    pub min_width: Option<u32>,
    pub min_height: Option<u32>,
    pub file_ext: Option<String>,
    pub camera_model: Option<String>,
    pub has_gps: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct SearchResult {
    pub id: MediaId,
    pub score: f32,
    pub file_path: PathBuf,
    pub file_name: String,
    pub media_type: MediaType,
    pub metadata: MediaMetadata,
    pub frame_info: Option<FrameInfo>,
}

#[derive(Clone, Debug)]
pub struct FrameInfo {
    pub source_video: PathBuf,
    pub frame_index: u32,
    pub timestamp_ms: u64,
}

// --- Config ---

#[derive(Clone, Debug, serde::Deserialize)]
pub struct Config {
    pub library: LibraryConfig,
    pub database: DatabaseConfig,
    pub embedding: EmbeddingConfig,
    pub extraction: ExtractionConfig,
    pub ingest: IngestConfig,
    pub server: ServerConfig,
}

// (sub-structs mirror the TOML structure in the HLD)
```

## 3. Component Internals

### 3.1 Scanner (`catalogy-scanner`)

```rust
pub trait Scanner {
    /// Walk directories and yield discovered files
    async fn scan(&self, paths: &[PathBuf]) -> Result<Vec<ScannedFile>>;

    /// Watch for filesystem changes and emit events
    async fn watch(&self, paths: &[PathBuf], tx: mpsc::Sender<FsEvent>) -> Result<()>;
}

pub enum FsEvent {
    Created(PathBuf),
    Modified(PathBuf),
    Deleted(PathBuf),
    Moved { from: PathBuf, to: PathBuf },
}
```

**Hashing strategy**: Stream file through SHA256 in 64KB chunks. For files > 1GB, optionally use a fast-path partial hash (first 1MB + last 1MB + file size) for initial screening, with full hash on demand.

**Concurrency**: Scanner spawns `tokio::task::spawn_blocking` for filesystem operations. Hashing is CPU-bound — use a bounded semaphore to limit concurrent hash tasks (default: 4).

### 3.2 Job Queue (`catalogy-queue`)

```rust
pub trait JobQueue {
    /// Enqueue a new job (idempotent — skips if same file_hash+stage exists and is not failed)
    async fn enqueue(&self, file_hash: &FileHash, file_path: &Path, stage: JobStage) -> Result<i64>;

    /// Claim the next pending job for a given stage
    async fn claim(&self, stage: JobStage, worker_id: &str) -> Result<Option<Job>>;

    /// Mark a job as completed
    async fn complete(&self, job_id: i64) -> Result<()>;

    /// Mark a job as failed with an error message
    async fn fail(&self, job_id: i64, error: &str) -> Result<()>;

    /// Get queue statistics
    async fn stats(&self) -> Result<QueueStats>;

    /// Enqueue re-embed jobs for all items with a different model_id
    async fn enqueue_reembed(&self, new_model_id: &str, new_model_version: &str) -> Result<u64>;
}
```

**Concurrency**: SQLite in WAL mode. Claims use `UPDATE ... WHERE status='pending' ORDER BY created_at LIMIT 1 RETURNING *` (atomic claim). The `worker_id` column prevents double-processing.

**Retry logic**: Jobs with `status='failed'` and `attempts < max_attempts` are re-claimable. Exponential backoff is handled by the worker (delay before re-claim), not the queue.

### 3.3 Frame Extractor (`catalogy-extract`)

```rust
pub trait FrameExtractor {
    /// Extract frames from a video file
    fn extract_frames(
        &self,
        video_path: &Path,
        strategy: &ExtractionStrategy,
    ) -> Result<Vec<ExtractedFrame>>;
}

pub enum ExtractionStrategy {
    /// Scene change detection with a max-interval floor
    /// This is the recommended default.
    Adaptive {
        /// ffmpeg scene detection threshold (0.0–1.0, lower = more sensitive). Default: 0.3
        scene_threshold: f32,
        /// Maximum seconds between frames even if no scene change. Default: 60
        max_interval_seconds: u32,
    },
    /// Extract one frame every N seconds (legacy/simple mode)
    Interval { seconds: u32 },
    /// Extract only keyframes (I-frames)
    Keyframes,
}
```

**Implementation**: Uses `ffmpeg-next` to decode video with a combined filter: scene-change detection (`select='gt(scene,T)'`) unioned with an interval floor (`select='not(mod(n,N))'`). This is applied during decode, so unselected frames are never fully decoded — minimal CPU overhead.

**Downscale during extraction**: Frames are extracted at a max dimension of **512px** (maintaining aspect ratio) using ffmpeg's hardware-accelerated scaler (`scale=512:512:force_original_aspect_ratio=decrease`). Rationale: CLIP resizes all inputs to 224×224 before inference, so anything above ~256px is lossless for embedding quality. Extracting at 512px (instead of native 4K) provides:
- **32× memory reduction** per frame (~0.75MB vs ~24MB for a 4K frame)
- **Faster extraction** — less data to decode and transfer
- **Dual purpose** — 512px frames serve as both embedding input and thumbnail source
- **No embedding quality loss** — the CLIP preprocessor would discard the extra resolution anyway

The 512px target gives headroom above the 224×224 model input so the center-crop step doesn't lose content at frame edges.

**Embedding-space dedup**: After embedding, consecutive frame vectors with cosine similarity > 0.95 are collapsed. Only the first frame in a similar run is kept. This catches visual redundancy that pixel-level scene detection misses (e.g., alternating camera angles in a news broadcast that look different in pixels but are semantically identical).

**Video-level aggregation**: After dedup, surviving frame embeddings are mean-pooled into a single video-level embedding stored as a `media_type=video` row. This enables "find the video" queries (matching the holistic content) alongside "find the moment" queries (matching individual frames).

**Memory guard**: For long videos (> 2 hours), process frames in chunks of 100 to avoid memory spikes.

### 3.4 Metadata Extractor (`catalogy-metadata`)

```rust
pub trait MetadataExtractor {
    /// Extract metadata from a media file
    async fn extract(&self, path: &Path, media_type: &MediaType) -> Result<MediaMetadata>;
}
```

**Image metadata**: `kamadak-exif` for EXIF. Falls back to `image` crate for basic dimensions if EXIF is missing.

**Video metadata**: Spawns `ffprobe -v quiet -print_format json -show_format -show_streams <path>` and parses JSON output. This avoids linking ffprobe into the binary. The ffprobe binary path is configurable.

### 3.5 Embedding Worker (`catalogy-embed`)

```rust
pub trait Embedder {
    /// Embed a batch of images
    fn embed_images(&self, images: &[&[u8]]) -> Result<Vec<Vec<f32>>>;

    /// Embed a text query
    fn embed_text(&self, text: &str) -> Result<Vec<f32>>;

    /// Model identity
    fn model_id(&self) -> &str;
    fn model_version(&self) -> &str;
    fn dimensions(&self) -> usize;
}
```

**ONNX session setup**:
```rust
let session = ort::Session::builder()?
    .with_execution_providers([
        ort::CoreMLExecutionProvider::default().build(),
        ort::CPUExecutionProvider::default().build(),
    ])?
    .commit_from_file(&model_path)?;
```

**Image preprocessing pipeline** (must match CLIP training):
1. Decode image (JPEG/PNG) → RGB
2. Resize to 224×224 (bicubic interpolation, center crop)
3. Convert to f32, scale to [0, 1]
4. Normalize: `(pixel - mean) / std` where mean=[0.48145466, 0.4578275, 0.40821073], std=[0.26862954, 0.26130258, 0.27577711]
5. Transpose to CHW format: [3, 224, 224]
6. Batch: [N, 3, 224, 224]

**Text preprocessing**: Tokenize with CLIP tokenizer (need to port or use a Rust tokenizer crate). Max sequence length 77. Pad with zeros.

**Batching**: Process images in batches of `config.embedding.batch_size`. The ONNX session handles the batch dimension natively.

### 3.6 Catalog (`catalogy-catalog`)

```rust
pub trait Catalog {
    /// Insert or update a media item
    async fn upsert(&self, item: &CatalogItem) -> Result<()>;

    /// Batch insert (for bulk ingest)
    async fn batch_upsert(&self, items: &[CatalogItem]) -> Result<()>;

    /// Search with vector + filters
    async fn search(&self, query: &SearchQuery, embedding: &[f32]) -> Result<Vec<SearchResult>>;

    /// Get a single item by ID
    async fn get(&self, id: &MediaId) -> Result<Option<CatalogItem>>;

    /// Get item by file hash
    async fn get_by_hash(&self, hash: &FileHash) -> Result<Option<CatalogItem>>;

    /// Mark item as deleted (tombstone)
    async fn soft_delete(&self, id: &MediaId) -> Result<()>;

    /// Update embedding for an existing item (re-embed)
    async fn update_embedding(&self, id: &MediaId, embedding: &Embedding) -> Result<()>;

    /// Build or rebuild the ANN index
    async fn build_index(&self) -> Result<()>;

    /// Get catalog statistics
    async fn stats(&self) -> Result<CatalogStats>;
}
```

**LanceDB search query construction**:
```rust
let results = table
    .vector_search(query_vector)
    .limit(query.limit)
    .nprobes(20)
    .filter(build_filter_expr(&query.filters))  // e.g. "media_type = 'image' AND file_modified > '2026-03-01'"
    .execute()
    .await?;
```

### 3.7 Search API (`catalogy-server`)

#### Endpoints

```
POST /api/search
  Request:  { "query": "sunset over ocean", "filters": { "media_type": "video", "after": "2026-01-01" }, "limit": 50 }
  Response: { "results": [{ "id": "...", "score": 0.87, "file_name": "DSC_1234.jpg", "media_type": "image", "thumb_url": "/api/thumb/...", ... }], "total": 50, "time_ms": 42 }

GET /api/media/:id
  Response: original media file (streaming, with range request support)

GET /api/thumb/:id
  Response: 300px JPEG thumbnail (generated on-demand, cached)

GET /api/stats
  Response: { "total_items": 1000000, "total_size_bytes": 4000000000000, "images": 800000, "videos": 50000, "video_frames": 150000, "index_status": "ready", "model_id": "clip-vit-h-14" }

GET /api/queue/status
  Response: { "pending": 0, "running": 2, "completed": 999998, "failed": 0 }

POST /api/admin/reembed
  Request:  { "model_path": "/path/to/new_model.onnx", "model_id": "clip-vit-l-14-336" }
  Response: { "jobs_queued": 1000000 }

POST /api/admin/rebuild-index
  Response: { "status": "started" }
```

#### Static Web UI

Embedded in the binary via `include_dir` or `rust-embed`. Single-page app with:
- Search bar with filter controls (media type, date range)
- Grid of thumbnails with scores
- Click-to-expand detail view with full metadata
- Infinite scroll / pagination

## 4. Error Handling Strategy

### Error types per crate

Each crate defines its own error enum using `thiserror`:

```rust
// catalogy-core
#[derive(Debug, thiserror::Error)]
pub enum CatalogyError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Embedding error: {0}")]
    Embedding(String),

    #[error("Extraction error: {0}")]
    Extraction(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("File not found: {path}")]
    FileNotFound { path: PathBuf },

    #[error("Unsupported format: {ext}")]
    UnsupportedFormat { ext: String },
}
```

### Recovery strategy

| Failure | Recovery |
|---|---|
| File disappeared between scan and process | Job marked `skipped`, logged as warning |
| Corrupt image (decode failure) | Job marked `failed`, skipped after max_attempts |
| ONNX inference error | Job retried up to 3 times, then `failed` |
| ffprobe/ffmpeg missing | Fatal on startup with helpful error message |
| LanceDB write failure | Retry with backoff; if persistent, pause ingest and alert |
| SQLite lock contention | WAL mode + busy_timeout(5000ms); if exceeded, retry |
| Disk full | Pause ingest, log error, expose via `/api/queue/status` |

### Graceful shutdown

On SIGINT/SIGTERM:
1. Stop accepting new jobs
2. Wait for in-progress jobs to complete (with 30s timeout)
3. Mark timed-out jobs as `pending` (will be reclaimed on next run)
4. Flush any buffered writes
5. Close database connections

## 5. Performance Considerations

### The real bottleneck: network I/O

Media files live on a NAS accessed via Wi-Fi backhaul (~25-50 MB/s effective). This makes the pipeline **I/O-bound, not compute-bound**. The M4 GPU will be idle waiting for bytes from the network most of the time.

| Operation | Bound by | Rate |
|---|---|---|
| Directory walk + stat | Network | ~1,000-5,000 files/sec |
| SHA256 hashing | **Network** (read) | ~25-50 MB/s from NAS (CPU can do 2 GB/s) |
| Frame extraction (video decode) | **Network** (read) | Limited by video read rate |
| CLIP embedding (frames in RAM) | Compute | ~60-80 images/sec on M4 CoreML |
| LanceDB writes | Local SSD | Not a bottleneck |

**Implication**: parallelizing compute (more embedding workers) has minimal benefit until the I/O pipeline is saturated. The architecture should focus on **keeping the network pipe full** via prefetching, and **minimizing re-reads** via local caching.

### Initial ingest time estimate (4TB over Wi-Fi backhaul)

At ~35 MB/s average throughput:
- **Reading 4TB**: ~32 hours of pure I/O time
- **Hashing**: bottlenecked by read speed, so same ~32 hours (overlaps with reading)
- **Frame extraction**: happens during the read pass, adds negligible overhead (frames cached locally)
- **Embedding**: ~60-80 imgs/sec × 4M items (1M images + ~3M frames) = ~14-18 hours, but runs concurrently with I/O
- **Realistic total**: ~2-4 days running continuously in background

With MoCA upgrade (~100 MB/s): ~1-2 days.

### Hashing strategy for NAS
Full SHA256 of every file over the network is expensive. Use a **fast-path skip**: if `(path, size, mtime)` are unchanged since last scan, skip the hash. Only hash new files or files where size/mtime changed. This reduces incremental re-scan of an unchanged library from hours to minutes.

### Embedding throughput
CLIP ViT-H-14 on M4 (CoreML): ~60-80 images/sec at batch_size=16. For 1M items: ~3.5-4.5 hours. With video frames (~3M additional frames): ~11-15 hours. Note: frames are pre-extracted at 512px max dimension, so the CLIP preprocessor's resize to 224×224 is a small-ratio downscale — negligible cost compared to inference.

### Memory budget (16 GB unified, shared with GPU)
- ONNX model: ~1.5 GB (ViT-H-14)
- CoreML model cache: ~500 MB (compiled model)
- Batch of 16 images at 224×224×3×f32: ~9 MB
- Extracted frames at 512px: ~0.75 MB each — 100 buffered = ~75 MB
- SQLite state DB: < 500 MB for 1M entries
- macOS system overhead: ~3-4 GB
- **Working set: ~6 GB total, leaving ~10 GB for macOS and other apps**

Important: M4 unified memory is shared between CPU and GPU. The ONNX model weights are accessed directly by the GPU without copying — this is an advantage of unified memory, not a limitation.

### Search latency
LanceDB with IVF-PQ at 1M vectors, nprobes=20: expected < 50ms for ANN. With scalar filter: < 100ms. Full hybrid query with metadata fetch: < 200ms target. Search is entirely local SSD — NAS not involved.
