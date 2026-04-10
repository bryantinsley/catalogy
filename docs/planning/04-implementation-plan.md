# Catalogy — Implementation Plan

## Guiding Principles

1. **Vertical slices** — each phase produces a working, testable artifact
2. **No dead code** — only build what the current phase uses
3. **Test from the start** — each component has unit tests; integration tests at phase boundaries
4. **Agent-friendly** — each task is self-contained with clear inputs, outputs, and acceptance criteria so any agent session can pick it up

---

## Phase 0: Project Skeleton & Build Infrastructure

**Goal**: Cargo workspace compiles, CI runs, basic config loads.

### Tasks

#### 0.1 Initialize Cargo workspace
- Create `Cargo.toml` workspace at repo root
- Create stub crates: `catalogy-core`, `catalogy-scanner`, `catalogy-queue`, `catalogy-extract`, `catalogy-metadata`, `catalogy-embed`, `catalogy-catalog`, `catalogy-search`, `catalogy-server`
- Each crate has `lib.rs` with a placeholder public function
- Binary crate at `src/main.rs` that just prints version
- **Acceptance**: `cargo build` and `cargo test` succeed with no errors

#### 0.2 Core types and config
- Implement `Config` struct with TOML deserialization in `catalogy-core`
- Implement all shared types from LLD section 2 (`MediaType`, `FileHash`, `ScannedFile`, `MediaMetadata`, `ExifData`, `Embedding`, `Job`, `SearchQuery`, etc.)
- Add `thiserror` error types
- Write a default `config.toml` template
- **Acceptance**: Can parse a config file and construct all core types. Unit tests pass.

#### 0.3 CLI skeleton
- Set up `clap` in `src/main.rs` with subcommands: `scan`, `ingest`, `search`, `status`, `reembed`, `serve`, `config`
- Each subcommand prints "not yet implemented" and exits
- **Acceptance**: `cargo run -- --help` shows all subcommands. `cargo run -- scan` prints stub message.

#### 0.4 CI setup
- GitHub Actions workflow: `cargo build`, `cargo test`, `cargo clippy`, `cargo fmt --check`
- **Acceptance**: CI passes on push to `main`

---

## Phase 1: Scanner + Job Queue (the "spine")

**Goal**: Can scan a directory of files, detect changes, and persist state. No processing yet — just discovery and job creation.

### Tasks

#### 1.1 SQLite state database setup
- Implement `catalogy-queue` crate
- Create/migrate SQLite schema (`files`, `jobs`, `models`, `config_state` tables)
- Use `rusqlite` with WAL mode
- Schema migration via simple version check (no ORM)
- **Acceptance**: Database created on first run. Schema matches LLD. Unit tests for create/migrate.

#### 1.2 File scanner
- Implement `catalogy-scanner` crate
- Recursive directory walker (using `walkdir` crate)
- Filter by configured extensions
- Streaming SHA256 hashing (`sha2` crate)
- Return `Vec<ScannedFile>`
- **Acceptance**: Given a directory with mixed files, returns only media files with correct hashes. Benchmark: hashes a 1GB file in < 1 second.

#### 1.3 Change detection
- Compare scanned files against `files` table in state DB
- Classify each file as: `New`, `Modified` (same path, different hash), `Moved` (same hash, different path), `Unchanged`, `Deleted` (in DB but not on disk)
- Update `files` table accordingly
- **Acceptance**: Unit test with mock state DB. Correctly detects all 5 states.

#### 1.4 Job enqueuing
- For `New` and `Modified` files, enqueue jobs for all stages
- For `Moved` files, update path in state DB and catalog (no re-processing)
- For `Deleted` files, enqueue tombstone job
- Idempotent: re-scanning doesn't create duplicate jobs
- **Acceptance**: Scan a directory twice — second scan produces zero new jobs.

#### 1.5 Wire up `scan` CLI command
- `catalogy scan --path /some/dir` runs scanner + change detection + job enqueuing
- `catalogy status` shows job queue stats (pending/completed/failed per stage)
- **Acceptance**: End-to-end: scan a test directory, verify jobs in SQLite, run status command.

#### 1.6 Watch mode (stretch)
- `catalogy scan --watch` uses `notify` crate to watch for filesystem changes
- On change event, re-hash the affected file and enqueue if needed
- **Acceptance**: Start watching, create a new file, verify job is enqueued within 2 seconds.

---

## Phase 2: Metadata Extraction

**Goal**: Process `extract_metadata` jobs. Catalog rows have all scalar metadata populated (no embeddings yet).

### Tasks

#### 2.1 Image metadata extraction
- Implement EXIF reading with `kamadak-exif`
- Extract dimensions with `image` crate (just header decode, not full image)
- Map to `MediaMetadata` / `ExifData` structs
- **Acceptance**: Given a JPEG with EXIF, extracts camera model, GPS, date taken. Given a PNG without EXIF, extracts dimensions only.

#### 2.2 Video metadata extraction
- Spawn `ffprobe -v quiet -print_format json -show_format -show_streams`
- Parse JSON into `MediaMetadata` fields
- Detect ffprobe binary on startup (configurable path, fallback to PATH)
- **Acceptance**: Given an MP4, extracts duration, resolution, fps, codec, bitrate.

#### 2.3 Job worker: extract_metadata
- Worker loop: claim `extract_metadata` job → extract → store metadata in intermediate format → mark complete
- Store extracted metadata in a SQLite side table (or pass forward in job payload) until catalog write in Phase 4
- Error handling: corrupt file → mark job failed
- **Acceptance**: Process 100 mixed image/video files. All jobs complete or correctly fail.

#### 2.4 Wire up `ingest` command (metadata only)
- `catalogy ingest --stages metadata` processes only metadata extraction jobs
- Progress bar (using `indicatif`)
- **Acceptance**: Scan → ingest metadata → status shows completed metadata jobs.

---

## Phase 3: Frame Extraction

**Goal**: Process `extract_frames` jobs. Video frames are extracted and available for embedding.

### Tasks

#### 3.1 Frame extractor with ffmpeg-next
- Implement `catalogy-extract` crate
- Open video → decode with ffmpeg filter pipeline → convert to RGB bytes at 512px max dimension
- **Default strategy: Adaptive** — scene-change detection (`select='gt(scene,0.3)'`) with max-interval floor (60s)
- Also support `Interval` (simple mode) and `Keyframes` strategies
- Memory guard: process max 100 frames per batch, yield between batches
- **Acceptance**: Extract frames from a 2-minute video with scene changes → frames cluster around scene transitions, not at fixed intervals. Static video → fewer frames than fast-cut video.

#### 3.2 Job worker: extract_frames
- Claim `extract_frames` job → extract → store frame data (temp files or in-memory) → enqueue downstream `embed` jobs for each frame
- For images: this stage is skipped (job created as `skipped`)
- **Acceptance**: Process 10 video files. Each produces variable frame count proportional to visual complexity.

#### 3.3 Thumbnail generation
- Generate 300px JPEG thumbnails from extracted frames or source images
- Cache to `~/.local/share/catalogy/thumbs/{id}.jpg`
- Utility function, used by both ingest and server
- **Acceptance**: Thumbnail exists after processing. JPEG, max dimension 300px.

---

## Phase 4: Embedding Pipeline

**Goal**: CLIP embeddings are generated for all items. Catalog rows in LanceDB are complete.

### Tasks

#### 4.1 CLIP model export to ONNX
- Python utility script (one-time): load ViT-H-14 from open_clip → export visual + text encoders to ONNX
- Validate exported model produces same outputs as PyTorch version
- Document the export process in `docs/model-export.md`
- **Acceptance**: Two ONNX files (visual encoder, text encoder). Output vectors match PyTorch within tolerance.

#### 4.2 ONNX Runtime integration
- Implement `catalogy-embed` crate
- Load ONNX session with CoreML → CPU fallback
- Image preprocessing pipeline (resize, normalize, CHW transpose) matching CLIP
- Batch inference
- Text encoding (tokenizer + text model)
- **Acceptance**: Embed a test image → 1024-dim vector. Embed "a photo of a cat" → 1024-dim vector. Cosine similarity between cat image and cat text > 0.25.

#### 4.3 CLIP tokenizer in Rust
- Port or integrate CLIP BPE tokenizer
- Options: `tokenizers` crate (HuggingFace), or ship the vocab + merge files and implement BPE
- **Acceptance**: Tokenize "a photo of a sunset" → same token IDs as Python CLIP tokenizer.

#### 4.4 LanceDB catalog setup
- Implement `catalogy-catalog` crate
- Create `media` table with schema from LLD
- Implement `upsert`, `batch_upsert`, `get`, `get_by_hash`
- **Acceptance**: Write 1000 rows, read them back. All fields round-trip correctly.

#### 4.5 Job worker: embed + index
- Claim `embed` job → load image/frame → embed → claim `index` job → write to LanceDB
- Batching: accumulate up to `batch_size` items, embed as batch, write as batch
- Tag each row with `model_id` + `model_version`
- **Acceptance**: End-to-end: scan 100 image files → ingest all stages → 100 rows in LanceDB with embeddings.

#### 4.6 Video frame dedup + aggregation
- After all frames for a video are embedded, run dedup: iterate frame vectors in timestamp order, drop any frame with cosine similarity > 0.95 to the previous kept frame
- Mean-pool surviving frame vectors into a video-level embedding
- Write one `media_type=video` row (aggregated embedding) + N `media_type=video_frame` rows (individual frames)
- **Acceptance**: A static 2-min video produces ~1-2 kept frames. A fast-cut montage retains most frames. Video-level embedding is the mean of kept frames.

#### 4.7 Build ANN index
- After bulk ingest, build IVF-PQ index on the `embedding` column
- `catalogy ingest --build-index` or automatic after ingest completes
- **Acceptance**: Index built. Search returns results (manually verified).

---

## Phase 5: Search

**Goal**: Working search via CLI and HTTP API.

### Tasks

#### 5.1 Query parsing
- Implement `catalogy-search` crate
- Parse user query into `SearchQuery` struct
- Support natural language text + optional structured filters
- Simple filter syntax: `type:video after:2026-01-01 "sunset"`
- **Acceptance**: Parse `type:image after:2026-01 sunset over ocean` → text="sunset over ocean", media_type=Image, after=2026-01-01.

#### 5.2 Hybrid search
- Encode text → vector via CLIP text encoder
- Build LanceDB query: ANN on vector + scalar WHERE clause from filters
- Return ranked `SearchResult` list
- **Acceptance**: Search "red car" on a catalog with car images → car images rank in top 10.

#### 5.3 CLI search
- `catalogy search "sunset over ocean" --limit 20 --type video`
- Tabular output (using `comfy-table` or similar)
- Show: rank, score, filename, media type, path
- **Acceptance**: Produces formatted table. Results are relevant.

#### 5.4 HTTP search API
- Implement `catalogy-server` with axum
- `POST /api/search` → JSON response
- `GET /api/media/:id` → stream original file (with Range header support for video)
- `GET /api/thumb/:id` → serve thumbnail
- `GET /api/stats` → catalog statistics
- **Acceptance**: `curl` against each endpoint returns expected responses. Video streaming works in browser.

#### 5.5 Web UI
- Single-page embedded UI (HTML/CSS/JS, no framework)
- Search bar → grid of thumbnail results → click to expand
- Filter controls for media type and date range
- Embedded in binary via `rust-embed`
- **Acceptance**: Open browser to localhost:8080, search works, thumbnails load, video plays.

---

## Phase 6: Re-embedding & Model Management

**Goal**: Can register a new embedding model and incrementally migrate the catalog.

### Tasks

#### 6.1 Model registry
- `catalogy reembed --register --model-path /path/to/new.onnx --model-id clip-vit-l-14-336 --dimensions 768`
- Inserts into `models` table
- Does NOT set as current yet
- **Acceptance**: Model registered in SQLite. Listed in `catalogy status`.

#### 6.2 Re-embed job creation
- `catalogy reembed --activate --model-id clip-vit-l-14-336`
- Scans catalog for rows where `model_id != new_model_id`
- Creates `re_embed` jobs for each
- Sets new model as `is_current` in models table
- **Acceptance**: For 1000-row catalog, creates 1000 re_embed jobs.

#### 6.3 Re-embed worker
- Process `re_embed` jobs: load original file → embed with new model → update LanceDB row (embedding + model_id + model_version)
- Can run concurrently with search (old embeddings still work, just lower quality)
- **Acceptance**: After re-embed completes, all rows have new model_id. Search still works.

#### 6.4 Index rebuild after migration
- Automatic or manual index rebuild after re-embedding completes
- `catalogy reembed --rebuild-index`
- **Acceptance**: ANN index rebuilt. Search latency matches pre-migration.

---

## Phase 7: Duplicate Detection

**Goal**: Surface exact, near-visual, and cross-video duplicates using data already collected by the pipeline.

### Tasks

#### 7.1 Exact duplicate detection
- Query `files` table for rows sharing the same `file_hash` but different `file_path`
- Group into duplicate sets with file sizes, paths, timestamps
- `catalogy dedup --tier exact` outputs a report
- **Acceptance**: Plant 3 identical files in different directories. Report lists them as a duplicate set.

#### 7.2 Near-visual duplicate detection
- For each item, find catalog neighbors with cosine similarity > `dedup.visual_similarity_threshold` (default 0.92)
- Exclude self-matches and video_frame → parent_video matches
- Group into clusters (transitive closure: if A~B and B~C, then {A,B,C} is one cluster)
- `catalogy dedup --tier visual` outputs report with similarity scores
- **Acceptance**: Two copies of the same photo at different resolutions/crops are detected. Two unrelated photos are not.

#### 7.3 Cross-video duplicate detection
- For each video's frame embeddings, search for high-similarity frames belonging to *different* videos
- Report pairs of videos that share significant frame overlap (e.g., > 30% of frames match)
- `catalogy dedup --tier cross-video` outputs report
- **Acceptance**: A video that contains a clip reused from another video is detected.

#### 7.4 Dedup UI integration
- Add `/api/dedup` endpoint returning duplicate clusters by tier
- Web UI "Duplicates" view: side-by-side thumbnails, similarity scores, keep/delete actions
- Actions are non-destructive by default (mark for deletion, require confirmation)
- **Acceptance**: Open web UI, browse duplicate clusters, select items to remove.

---

## Phase 8: Video Transcoding

**Goal**: Normalize videos exceeding configured quality thresholds, reclaiming storage from unnecessarily large files.

### Tasks

#### 8.1 Transcode decision engine
- After metadata extraction, evaluate each video against `[transcode]` config thresholds
- Decision matrix: skip (already within spec), transcode (exceeds resolution/bitrate/codec), or skip (codec already optimal)
- Enqueue `transcode` job only when transcoding would actually reduce file size meaningfully
- **Acceptance**: A 4K ProRes video is flagged for transcode. A 720p H.265 video is skipped. Config with `enabled = false` skips all.

#### 8.2 ffmpeg transcode worker
- Read source from NAS → transcode to local SSD staging → verify → apply original_policy
- Use VideoToolbox hardware encoder on Apple Silicon (`-c:v hevc_videotoolbox`) when `use_hw_encoder = true`
- Fall back to software encoder (`libx265`) if hw encoder unavailable
- Preserve all metadata (creation dates, etc.) in output
- **Acceptance**: Transcode a 4K H.264 video to 1080p H.265. Output plays correctly, duration matches, metadata preserved.

#### 8.3 Transcode verification
- Compare original vs. transcode: duration (within 0.5s), frame count (within 1%), audio stream count matches
- If verification fails, mark job as failed, do NOT apply original_policy
- Log space savings per file and cumulative
- **Acceptance**: Truncated transcode (simulated by killing ffmpeg early) fails verification. Original is preserved.

#### 8.4 Original policy enforcement
- `keep`: both files coexist, catalog tracks both paths (original + transcode)
- `archive`: move original to `archive_dir` preserving directory structure
- `replace`: delete original after verification passes, update catalog path
- **Acceptance**: Each policy mode works correctly. `replace` only runs after verification.

#### 8.5 Transcode reporting
- `catalogy transcode --dry-run` shows what would be transcoded and estimated space savings
- `catalogy status` shows transcode queue progress and cumulative savings
- **Acceptance**: Dry run accurately predicts space savings within 20%.

#### 8.6 Re-embed after transcode
- If original_policy is `replace`, the file hash changes. Update catalog entry and re-embed if needed.
- If `keep`, the transcode is a new file — optionally index it separately or treat as an alias.
- **Acceptance**: After `replace` transcode, search still finds the video by semantic content.

---

## Phase 9: Production Hardening

**Goal**: Ready for sustained use on the full 4TB library.

### Tasks

#### 9.1 Graceful shutdown
- SIGINT/SIGTERM handler
- Drain in-progress jobs (30s timeout)
- Mark timed-out jobs as pending
- **Acceptance**: Kill process during ingest → restart → no duplicate processing, no lost data.

#### 9.2 Progress reporting
- Real-time progress during ingest (items/sec, ETA, stage breakdown)
- Persistent progress (survives restart via job queue stats)
- **Acceptance**: Progress bar during 1000-item ingest shows accurate ETA.

#### 9.3 Logging and observability
- Structured logging with `tracing` crate
- Log levels: ERROR for failures, WARN for skips, INFO for milestones, DEBUG for per-item
- Log to file + stderr
- **Acceptance**: Can diagnose a failed job from log output alone.

#### 9.4 Memory profiling and optimization
- Profile memory during 10K-item ingest
- Ensure < 4GB peak
- Optimize any hot spots (image decode buffers, LanceDB write batches)
- **Acceptance**: `heaptrack` shows peak < 4GB.

#### 9.5 Large-scale testing
- Test with 100K items (subset of real library)
- Full pipeline: scan → extract → embed → index → search
- Measure and document actual throughput numbers
- **Acceptance**: Completes without OOM, crashes, or data corruption. Throughput documented.

---

## Dependency Summary

```
Phase 0 ──→ Phase 1 ──→ Phase 2 ──→ Phase 3 ──→ Phase 4 ──→ Phase 5 ──→ Phase 6 ──┬─→ Phase 9
skeleton     scanner     metadata    frames      embedding    search      reembed    │   hardening
             job queue                            LanceDB     HTTP API               │
                                                              Web UI                 │
                                                                                     │
                                                 Phase 4 ──→ Phase 7 (dedup)  ───────┤
                                                              needs embeddings       │
                                                                                     │
                                                 Phase 3 ──→ Phase 8 (transcode) ───┘
                                                              needs metadata/ffmpeg
```

Phases 7 (dedup) and 8 (transcode) can be developed in parallel with Phase 6, since they only depend on earlier phases. All three feed into Phase 9 (hardening).

Phases 2 and 3 can be developed in parallel (both depend on Phase 1, neither depends on the other).

---

## Key Crate Dependencies

| Crate | Purpose | Used in |
|---|---|---|
| `clap` | CLI argument parsing | main binary |
| `serde`, `toml` | Config deserialization | catalogy-core |
| `thiserror` | Error types | all crates |
| `tokio` | Async runtime | all crates |
| `rusqlite` | SQLite (state DB, job queue) | catalogy-queue |
| `walkdir` | Directory traversal | catalogy-scanner |
| `sha2` | SHA256 hashing | catalogy-scanner |
| `notify` | Filesystem watcher | catalogy-scanner |
| `kamadak-exif` | EXIF reading | catalogy-metadata |
| `image` | Image decode/resize/thumbnails | catalogy-metadata, catalogy-embed |
| `ffmpeg-next` | Video frame extraction | catalogy-extract |
| `ort` | ONNX Runtime (CLIP inference) | catalogy-embed |
| `lancedb` | Vector + scalar catalog | catalogy-catalog |
| `arrow` | Arrow data types (LanceDB interop) | catalogy-catalog |
| `axum`, `tower` | HTTP server | catalogy-server |
| `rust-embed` | Static file embedding | catalogy-server |
| `tracing` | Structured logging | all crates |
| `indicatif` | Progress bars | main binary |
| `uuid` | UUIDv7 generation | catalogy-core |
| `chrono` | Date/time handling | catalogy-core |
| `comfy-table` | CLI table output | main binary |
| `tokenizers` | CLIP BPE tokenizer | catalogy-embed |

---

## Agent Context Protocol

When starting a new agent session to work on Catalogy, load these files in order:

1. **`docs/planning/01-one-pager.md`** — scope and constraints
2. **`docs/planning/02-hld.md`** — architecture and component overview
3. **`docs/planning/03-lld.md`** — schemas, types, APIs
4. **`docs/planning/04-implementation-plan.md`** — this file; find the current phase/task
5. **`Cargo.toml`** — workspace structure (once it exists)
6. **The specific crate being worked on** — read its `lib.rs` and tests

### Determining current state

Run `catalogy status` (once it exists) or check:
- Which crates exist and have non-stub code
- Which phases have passing tests
- Job queue stats (if state DB exists)

### Task handoff format

When finishing a session, leave a note in `docs/planning/progress.md`:
```
## Session YYYY-MM-DD
- Completed: Phase X, Task X.Y — description of what was done
- In progress: Phase X, Task X.Z — what's started but not finished
- Blockers: any issues encountered
- Next: what to pick up next
```
