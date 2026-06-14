# Progress Log

## Session 2026-06-14

- **Completed: Phase 4, Task 4.6 — Video frame dedup + aggregation.**
  The embed pipeline previously only worked for images; videos extracted frames
  but nothing video-related ever reached the catalog. Two root causes, both
  fixed:
  1. `catalogy-extract/src/worker.rs` — `extract_frames` returns a `TempDir`
     that was bound to `_temp_dir` and dropped at the end of `process_single_job`,
     deleting every extracted frame before the embed stage could read it. Added
     `persist_frames()` to copy frames into a durable per-video cache
     (`<thumb_dir>/frames/<file_hash>/`) and a public `read_frame_metadata()`
     reader for the `.frames` sidecar.
  2. `catalogy-embed/src/worker.rs` — `run_embed_worker` called
     `embed_image(file_path)` on the raw video path (CLIP can't embed an .mp4).
     Rewrote it to branch on media type: images embed directly; videos read the
     persisted frames, embed each, dedup in embedding space (`dedup_frames`,
     cosine > 0.95), write one `video_frame` row per kept frame plus one
     mean-pooled `video` row (`aggregate_video_frames`/`mean_pool`, previously
     dead code). Signature gained `frames_meta_dir: &Path` and
     `dedup_threshold: f32`; both call sites (CLI `src/main.rs`, server
     `crates/catalogy-server/src/api.rs`) updated to pass the same thumbnails
     dir their extract stage wrote to.

  **Verified** end-to-end via the CLI against an isolated `XDG_DATA_HOME` with
  the real ViT-H-14 models: scan → ingest (frames+metadata+embed) → search on 5
  sample clips. All 5 embed jobs complete; search returns both `video` and
  `video_frame` rows ranked by CLIP similarity; multi-frame videos yield
  multiple distinct deduped frame rows; short clips keep ~1 frame (matches the
  4.6 acceptance criteria). New unit tests: `persist_frames`, frame-sidecar
  round-trip, missing-sidecar.

- **Discovered (not yet addressed):**
  - The exported `visual.onnx` has a **fixed batch dimension of 1** —
    `EmbedSession::embed_images` (batched) fails with a Reshape error on >1
    frame. The embed worker therefore embeds frames one at a time. Re-exporting
    the model with a dynamic batch axis (Phase 4.1) would let us batch and speed
    up embedding.
  - `run_reembed_worker` (Phase 6) still embeds the raw file path and has the
    same video limitation; not exercised yet, left for the re-embed phase.
  - CLI `search --type` flag is actually `--media-type` (help text says `type`);
    pre-existing, unrelated.
  - File `size` is 0 in some surfaces (scan-side `files` table / transcode
    table); the catalog rows themselves get the correct size from the embed
    worker. Separate scan/metadata issue.

- **Next:** decide whether to deploy this to the systemd service (rebuild
  `~/.cargo/bin/catalogy`, restart) and run it against the real library, then
  commit/push the `phase4-video-embed` branch. Consider Phase 4.7 (build ANN
  index) once row counts grow — currently search is brute-force (fine for small
  catalogs).

## Session 2026-06-14 (cont.) — search scoring + GPU

- **Search now reports true cosine similarity.** `search_vector` used LanceDB's
  default L2 metric and the engine reported `1/(1+squared_L2)`, compressing the
  meaningful range (relevant ~0.41 vs irrelevant ~0.35). Set
  `DistanceType::Cosine` on the query and report `1 - distance` across all three
  callers (search engine + visual/cross-video dedup). Scores are now
  interpretable cosine (relevant ~0.28, unrelated ~0.07) and consistent with the
  dedup/near-visual thresholds.

- **GPU (CUDA) inference wired and deployed.** ViT-H ran on CPU because no CUDA
  EP was registered and `ort` lacked the feature. Added a `cuda` cargo feature
  (`catalogy` → `catalogy-embed` → `ort/cuda`) and an `execution_providers()`
  helper in `session.rs` selected at runtime by `CATALOGY_EXECUTION_PROVIDER`
  (default cpu; cuda picks `CATALOGY_CUDA_DEVICE`, falls back to CPU). Verified
  on the RTX 3090 (the Blackwell is saturated by vLLM): embed ran 5× faster even
  on a trivial 5-clip workload (2s vs 10s); the service is GPU-resident (~5.4 GB).

  **Deployment is not just a binary copy — three machine-local steps:**
  1. Build with the feature: `cargo build --release --features cuda`.
  2. Copy the ORT provider libs next to the installed binary — ONNX Runtime
     loads `libonnxruntime_providers_{shared,cuda}.so` from the executable's
     own directory, so `cp -L target/release/*.so* ~/.cargo/bin/` is required
     (CPU works without them; CUDA fails with "cannot open shared object file").
  3. systemd env (`~/.config/catalogy/catalogy.env`): `CATALOGY_EXECUTION_PROVIDER=cuda`,
     `CATALOGY_CUDA_DEVICE=0`, `CUDA_VISIBLE_DEVICES=<3090 UUID>`,
     `LD_LIBRARY_PATH=/usr/local/cuda-12.8/lib64:~/.local/lib/ollama/mlx_cuda_v13`
     (cuDNN 9 currently borrowed from ollama's bundle — a proper cuDNN install
     would remove that fragile dependency).

  Not committed to the repo: the env file and the provider-lib copy are
  machine-local. The code changes (Cargo.toml ×2, session.rs) are committed.

- **Still open:** fixed-batch-1 visual.onnx (re-export for batching, now extra
  worthwhile on GPU); ANN index (Phase 4.7); API `media_type` filter also
  returns `video_frame` rows; reembed worker video gap (Phase 6).
