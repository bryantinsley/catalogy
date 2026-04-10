# Catalogy — High-Level Design

## 1. System Overview

Catalogy is a local-first, offline media catalog and search engine built in Rust. It processes a large media library through a multi-stage pipeline (scan → extract → embed → index) and exposes hybrid search (semantic + metadata) via an HTTP API and CLI.

```
                          ┌──────────────────────────┐
                          │       CLI / Config        │
                          │  (clap, toml config)      │
                          └────────────┬─────────────┘
                                       │
                 ┌─────────────────────┼─────────────────────┐
                 │                     │                     │
                 ▼                     ▼                     ▼
        ┌────────────────┐  ┌──────────────────┐  ┌────────────────┐
        │    Scanner      │  │   Search API     │  │   Admin API    │
        │  (fs watcher    │  │  (axum server)   │  │  (re-embed,    │
        │   + hasher)     │  │                  │  │   stats, etc.) │
        └───────┬────────┘  └────────┬─────────┘  └───────┬────────┘
                │                    │                     │
                ▼                    │                     │
        ┌────────────────┐           │                     │
        │   Job Queue    │           │                     │
        │   (SQLite)     │◄──────────┼─────────────────────┘
        └───────┬────────┘           │
                │                    │
     ┌──────────┼──────────┐         │
     ▼          ▼          ▼         │
┌─────────┐┌─────────┐┌─────────┐   │
│ Frame   ││Metadata ││Embedding│   │
│Extractor││Extractor││ Worker  │   │
│(ffmpeg) ││(exif/   ││(ort/    │   │
│         ││ffprobe) ││ CLIP)   │   │
└────┬────┘└────┬────┘└────┬────┘   │
     │          │          │         │
     └──────────┼──────────┘         │
                ▼                    │
        ┌────────────────┐           │
        │    LanceDB     │◄──────────┘
        │   (catalog)    │
        └────────────────┘
```

## 2. Components

### 2.1 Scanner

**Purpose**: Discover media files and detect changes.

- Walks the configured media directories recursively
- Computes SHA256 of each file (streaming, not loading full file into memory)
- Compares against the state database to determine: new, changed, moved, or deleted
- Enqueues processing jobs for new/changed files
- Supports both one-shot scan and continuous watch mode (`notify` crate)

**File type detection**: By extension initially (jpg, jpeg, png, webp, gif, bmp, tiff, mp4, mov, avi, mkv, webm). Can be extended with `infer` crate for magic-byte detection.

### 2.2 Job Queue

**Purpose**: Track processing state across pipeline stages. Enable resumable, incremental processing.

**Backed by SQLite** (via `rusqlite`), separate from LanceDB. This is purely operational state — not the catalog.

**Job stages**:
1. `extract_frames` — video only; extract keyframes at configurable interval
2. `extract_metadata` — read EXIF, ffprobe, file stats
3. `embed` — generate CLIP embedding
4. `index` — write final row to LanceDB catalog
5. `transcode` — optional; normalize video to target format/resolution

Each stage is independently retriable. A job can fail at stage 2 and be retried without re-doing stage 1.

**Special job types**:
- `re_embed` — triggered when a new embedding model is configured. Scans existing catalog rows with old `model_id` and queues them for re-embedding.
- `transcode` — triggered when transcoding is enabled and a video exceeds the configured max resolution/bitrate/codec thresholds.

### 2.3 Frame Extractor

**Purpose**: Extract representative frames from video files.

- Uses `ffmpeg-next` (Rust bindings to libav*)
- **Default strategy: adaptive** — scene-change detection (ffmpeg `select='gt(scene,0.3)'`) with a max-interval floor (at least 1 frame per 60s). This captures visual transitions while ensuring static videos still get coverage.
- Frames extracted at **512px max dimension** (CLIP input is 224px; 512 serves dual purpose as thumbnail source)
- After embedding, **dedup in vector space** — drop frames with cosine similarity > 0.95 to an already-kept frame
- **Mean-pool surviving frame vectors** into a single video-level embedding for "find the video" queries
- Stores both: individual frame rows (`video_frame`) + one aggregated video row (`video`) in the catalog

### 2.4 Metadata Extractor

**Purpose**: Extract structured metadata from media files.

| Source | Fields |
|---|---|
| File system | path, size, created, modified, extension |
| EXIF (images) | camera model, GPS, date taken, orientation, focal length, ISO |
| ffprobe (video) | duration, resolution, fps, codec, bitrate, audio tracks |
| Derived | aspect ratio, megapixels, file type category |

Uses `kamadak-exif` or `rexiv2` for EXIF, and spawns `ffprobe` as a subprocess (JSON output mode) for video metadata.

### 2.5 Embedding Worker

**Purpose**: Generate vector embeddings for visual content.

- Loads a CLIP model exported to ONNX format
- Uses the `ort` crate (ONNX Runtime bindings) with CoreML execution provider on macOS
- Processes images in batches (configurable batch size, default 8–16)
- Image preprocessing: resize to 224×224, normalize with CLIP mean/std, convert to tensor
- Outputs 1024-dim f32 vectors (for ViT-H-14)
- Model versioning: each embedding is tagged with `(model_id, model_version)`

### 2.6 LanceDB Catalog

**Purpose**: Persistent storage for the media catalog — vectors + scalar metadata.

**Why LanceDB**:
- Written in Rust, native SDK
- File-based, no server process
- Supports ANN indexes (IVF-PQ) for fast vector search at scale
- Supports scalar filters in the same query (hybrid search)
- Supports multiple vector columns (useful during model migration)
- Column-oriented (Lance format) — efficient for selective reads

**Table design**: Single `media` table with vector columns and scalar columns. See LLD for full schema.

**Indexing**: IVF-PQ index built after initial bulk ingest, rebuilt periodically or on-demand.

### 2.7 Search API

**Purpose**: HTTP server for search and browsing.

- Built with `axum` (Tokio-based, async)
- Endpoints:
  - `POST /search` — hybrid query (text → vector + optional filters)
  - `GET /media/:id` — serve original media file
  - `GET /thumb/:id` — serve/generate thumbnail
  - `GET /stats` — catalog statistics
  - `GET /` — web UI (embedded static assets)
- CLIP text encoder loaded once at startup for query encoding
- Search returns ranked results with metadata + thumbnails

### 2.8 Duplicate Detector

**Purpose**: Surface duplicate and near-duplicate media across the library.

Three tiers, all derived from data the pipeline already produces:

| Tier | Method | Detects | Extra cost |
|---|---|---|---|
| **Exact** | SHA256 hash match | Byte-identical copies (same file in multiple folders) | Zero — hash already computed by scanner |
| **Near-visual** | CLIP embedding cosine similarity > configurable threshold (default 0.92) | Same scene at different resolutions, crops, re-encodes, formats | Zero — embeddings already in LanceDB |
| **Cross-video** | Frame embedding overlap between different videos | Shared footage / reused clips across videos | Zero — frame embeddings already indexed |

- Exposed via `catalogy dedup` CLI command (report mode, interactive review mode)
- Also surfaced in web UI as a "Duplicates" view
- **Non-destructive by default** — reports only. User chooses which to keep/delete/archive.
- For exact dupes: shows all paths, file sizes, timestamps to help decide which to keep
- For near-visual: shows side-by-side thumbnails with similarity score

### 2.9 Video Transcoder

**Purpose**: Normalize videos to a configurable maximum quality standard, reducing storage waste from unnecessarily high-resolution or inefficient codecs.

- Uses ffmpeg (already a dependency for frame extraction)
- **Trigger**: during ingest, after metadata extraction reveals the video exceeds configured thresholds (resolution, bitrate, or codec)
- **Configurable policy**:
  - `max_resolution`: e.g., `1080p` — videos above this get downscaled
  - `target_codec`: e.g., `h265` (HEVC) — more efficient than H.264 at same quality
  - `target_crf`: e.g., `23` — perceptual quality target (lower = better quality, bigger file)
  - `original_policy`: `keep` (side-by-side), `archive` (move to archive dir), or `replace` (delete original after verification)
- **I/O-aware**: reads the original from NAS once, writes the transcode to a staging area on local SSD, then copies back to NAS (or a designated transcode output directory). Avoids reading the file twice.
- **Verification**: after transcoding, compares duration and frame count between original and transcode to catch truncation. Only applies `original_policy` after verification passes.
- **Apple Silicon advantage**: ffmpeg with VideoToolbox hardware encoder on M4 can transcode H.265 at near-realtime speed with minimal CPU load.
- **Non-destructive by default**: `original_policy` defaults to `keep`. User must explicitly opt into `archive` or `replace`.

### 2.10 CLI

**Purpose**: Command-line interface for all operations.

```
catalogy scan [--watch] [--path <dir>]     # scan / watch for changes
catalogy ingest [--workers N]              # process job queue
catalogy search <query> [--limit N] [--type image|video] [--after DATE]
catalogy status                            # show queue stats, catalog size
catalogy dedup [--report] [--tier exact|visual|cross-video]  # find duplicates
catalogy transcode [--dry-run] [--path <dir>]  # transcode videos exceeding thresholds
catalogy reembed --model <path>            # queue re-embedding with new model
catalogy serve [--port 8080]               # start HTTP API
catalogy config                            # show/edit config
```

## 3. Data Flow

### 3.1 Initial Ingest
```
Media directory
    │
    ▼ Scanner (walk + hash)
    ▼
Job Queue (SQLite)
    │
    ├─ Images ─────────────────────┐
    │                              │
    ├─ Videos                      │
    │   │ stage: extract_frames    │
    │   ▼ Frame Extractor          │
    │   │ (adaptive: scene-change  │
    │   │  + interval floor,       │
    │   │  downscaled to 512px)    │
    │   │                          │
    │   │ stage: extract_metadata  │
    │   ▼ Metadata Extractor       │
    │                              │
    │ stage: embed                 │
    ▼ Embedding Worker (CLIP/ONNX) │
    │                              │
    │ stage: dedup_frames          │
    ▼ Cosine similarity > 0.95     │
    │ → drop redundant frames      │
    │                              │
    │ stage: aggregate             │
    ▼ Mean-pool surviving frame    │
    │ vectors → video-level embed  │
    │                              │
    │ stage: index                 │
    ▼ LanceDB                      │
      ├─ video row (aggregated)    │
      └─ video_frame rows (kept)   │
                                   │
      ← images indexed here too ───┘
```

### 3.2 Incremental Update
```
fs event (new/modified file)
    │
    ▼ Scanner
    │ hash → compare with state DB
    │ if new/changed: enqueue jobs
    │ if moved: update path in catalog
    │ if deleted: mark tombstone in catalog
    ▼
Job Queue → pipeline as above
```

### 3.3 Search
```
User query: "sunset over ocean, video, last 30 days"
    │
    ▼ Parse query
    │ text: "sunset over ocean"
    │ filters: type=video, modified > 30d ago
    │
    ▼ CLIP text encoder → 1024-dim vector
    │
    ▼ LanceDB hybrid query
    │ ANN(vector, top_k=100) WHERE type='video' AND modified > '2026-03-10'
    │
    ▼ Ranked results with metadata + thumbnail paths
```

### 3.4 Re-embedding
```
New model configured
    │
    ▼ Admin CLI: `catalogy reembed --model new_clip_v2.onnx`
    │
    ▼ Scan catalog for rows where model_id != new_model
    │ Enqueue re_embed jobs
    │
    ▼ Embedding Worker (with new model)
    │ Read original file → embed → update vector column
    │ Update model_id, model_version on row
    │
    ▼ Optionally: rebuild ANN index
```

## 4. Key Design Decisions

### D1: SQLite job queue separate from LanceDB catalog
**Rationale**: The job queue is high-write, low-read operational state (status updates, retries, progress tracking). LanceDB is optimized for analytical/vector workloads. Mixing concerns would compromise both. SQLite handles the transactional job state perfectly.

### D2: Content-addressed (SHA256) change detection
**Rationale**: Filesystem timestamps are unreliable (copies, restores, network mounts). Hashing is authoritative. Streaming hash means we never load a 4GB video into memory. This also enables move detection (same hash, different path).

### D3: ONNX Runtime over native PyTorch/candle
**Rationale**: ONNX provides a stable model interchange format. Decouples the embedding model (which originates in Python/PyTorch) from the Rust runtime. The `ort` crate supports CoreML (Apple Silicon), CUDA, and CPU execution providers. Model upgrades become: export new model → swap ONNX file → re-embed.

### D4: Single LanceDB table with model versioning columns
**Rationale**: Rather than a separate table per embedding model, store `model_id` and `model_version` as scalar columns. During migration, old and new embeddings coexist. Search filters to `model_id = current`. Once migration completes, old vectors can be compacted away. This avoids table management complexity and supports gradual rollover.

### D5: Adaptive frame extraction → dedup → video-level aggregation
**Rationale**: Fixed-interval extraction (1 frame/30s) is content-blind — it over-samples static video and under-samples fast-paced content. Scene-change detection adapts to content naturally (ffmpeg computes it during decode for near-zero cost). However, pixel-level scene detection can still produce semantically redundant frames (e.g., alternating camera angles that differ in pixels but not meaning). Deduplicating in embedding space (cosine similarity > 0.95) catches this. Finally, mean-pooling the surviving frame vectors into a video-level embedding gives a cohesive "what is this video about" representation. This produces two searchable layers: "find the video" (video row) and "find the moment" (frame rows).

### D6: Frame caching is optional
**Rationale**: For 1M items, caching extracted frames to disk could consume hundreds of GB. Default mode: extract → embed → discard frames. Optionally persist frames for thumbnail serving. Thumbnails can also be generated on-demand from source files.

### D7: axum for HTTP, clap for CLI
**Rationale**: Both are the Rust ecosystem standards — well-maintained, async-native (axum with Tokio), and widely used. No need to be clever here.

## 5. Deployment Context & I/O Strategy

### Hardware
- **Mac Mini M4** — 10-core GPU, 16-core Neural Engine, 16 GB unified memory
- **No eGPU option** — Apple Silicon dropped Thunderbolt eGPU support entirely. The M4's native GPU + ANE are the only compute targets.
- **M4 embedding throughput** — CLIP ViT-H-14 via CoreML: ~60-80 images/sec. This is not the bottleneck.

### Network Topology
```
NAS ──(Ethernet)──► Orbi AX6000 Router ──(Wi-Fi 6 backhaul)──► Orbi Satellite ──(Ethernet)──► Mac Mini M4
```
Effective throughput to NAS: **~200-400 Mbps** (~25-50 MB/s). The wireless backhaul between Orbi units is the constraint. A MoCA adapter pair (~$120) on existing coax would upgrade this to ~1 Gbps.

### I/O-Aware Pipeline Design

The pipeline treats NAS access as **expensive and unreliable**. All catalog data, indexes, thumbnails, and embeddings live on the Mac Mini's local SSD. The NAS is only accessed for:
1. **Initial scan** — walking directories and reading file metadata
2. **Frame extraction** — reading video bytes to extract keyframes
3. **Metadata extraction** — EXIF reads, ffprobe

After initial ingest, the NAS is only hit for **new/changed files** and **"open original"** requests from the UI.

#### Design principles
1. **Scan-once, process-locally** — extracted frames and thumbnails are cached to local SSD. Embedding and indexing never touch the network.
2. **Background trickle ingest** — the initial 4TB ingest runs as a background daemon, processing files at whatever rate the network allows. No urgency, no timeouts.
3. **Prefetch pipeline** — while the embedder processes batch N, the extractor prefetches batch N+1 from NAS. Hides network latency behind compute.
4. **Checkpointing** — every file's processing state is tracked in SQLite. If the connection drops or the process restarts, it resumes exactly where it left off. No wasted work.
5. **Graceful degradation** — if the NAS is unreachable, search and browsing still work (local index + local thumbnails). Only new ingestion pauses.

#### Storage budget (local SSD)
| Data | Estimated size |
|---|---|
| LanceDB catalog (1M × 1024-dim f32 embeddings + metadata) | ~5 GB |
| SQLite state DB | < 500 MB |
| Thumbnails (1M × ~30KB JPEG) | ~30 GB |
| ONNX models | ~2 GB |
| **Total** | **~38 GB** |

This fits comfortably on the Mac Mini's internal SSD, leaving the NAS as a pure media source that can go offline without affecting search.

## 6. Configuration

TOML config file at `~/.config/catalogy/config.toml` (overridable via `--config`):

```toml
[library]
paths = ["/Volumes/Media/Photos", "/Volumes/Media/Videos"]
extensions_image = ["jpg", "jpeg", "png", "webp", "gif", "bmp", "tiff"]
extensions_video = ["mp4", "mov", "avi", "mkv", "webm"]

[database]
catalog_path = "~/.local/share/catalogy/catalog.lance"
state_path = "~/.local/share/catalogy/state.db"

[embedding]
model_path = "~/.local/share/catalogy/models/clip-vit-h-14.onnx"
model_id = "clip-vit-h-14"
model_version = "1"
dimensions = 1024
batch_size = 16
execution_provider = "coreml"  # or "cuda", "cpu"

[extraction]
frame_strategy = "adaptive"       # "adaptive" (recommended), "interval", or "keyframes"
scene_threshold = 0.3             # for adaptive: ffmpeg scene detection sensitivity (0.0–1.0)
max_interval_seconds = 60         # for adaptive: floor interval to ensure coverage of static video
frame_interval_seconds = 30       # for interval mode only
frame_max_dimension = 512         # downscale during decode (CLIP input is 224px, 512 gives crop headroom)
dedup_similarity_threshold = 0.95 # cosine similarity above which frames are considered redundant

[ingest]
workers = 4
hash_algorithm = "sha256"

[dedup]
visual_similarity_threshold = 0.92  # cosine similarity for near-visual duplicate detection
cross_video_threshold = 0.90        # cosine similarity for cross-video frame overlap

[transcode]
enabled = false                     # opt-in; must be explicitly enabled
max_resolution = "1080p"            # videos above this get downscaled (e.g., "1080p", "1440p", "4k")
target_codec = "h265"               # target codec ("h265", "h264", "av1")
target_crf = 23                     # quality (18=visually lossless, 23=good, 28=smaller)
target_container = "mp4"            # output container format
audio_codec = "aac"                 # audio codec ("aac", "copy" to keep original)
audio_bitrate = "128k"              # audio bitrate (ignored if audio_codec = "copy")
use_hw_encoder = true               # use VideoToolbox on macOS for hardware-accelerated H.265
original_policy = "keep"            # "keep" (both versions), "archive" (move original to archive dir), "replace" (delete after verify)
archive_dir = ""                    # for "archive" policy: where to move originals
staging_dir = "~/.local/share/catalogy/transcode_staging"  # local SSD temp space for transcoding

[server]
port = 8080
host = "127.0.0.1"
```

## 7. Crate Structure

```
catalogy/
├── Cargo.toml              # workspace root
├── crates/
│   ├── catalogy-core/      # shared types, config, errors
│   ├── catalogy-scanner/   # file discovery + hashing
│   ├── catalogy-queue/     # SQLite job queue
│   ├── catalogy-extract/   # frame extraction (ffmpeg)
│   ├── catalogy-metadata/  # EXIF + ffprobe extraction
│   ├── catalogy-embed/     # ONNX/CLIP embedding
│   ├── catalogy-catalog/   # LanceDB read/write
│   ├── catalogy-search/    # query parsing + hybrid search
│   └── catalogy-server/    # axum HTTP server
├── src/
│   └── main.rs             # CLI entry point (clap)
└── docs/
    └── planning/
```

Workspace with internal crates. Each crate has a focused concern, can be tested independently, and compiles in parallel.
