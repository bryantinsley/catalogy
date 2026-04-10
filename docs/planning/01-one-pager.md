# Catalogy — One-Pager

## Problem

We have a media library of ~1 million items (~4TB) containing images and videos. Finding specific content requires manually browsing or remembering file paths. There is no way to search by visual content, scene attributes, or rich metadata. The current Python PoC proved the concept with CLIP-based semantic search but cannot scale to the full library, lacks incremental processing, and is difficult to maintain long-term.

## Solution

Rewrite Catalogy as a Rust-native media catalog and search engine that:

- **Incrementally ingests** a large media library using content-addressed tracking (SHA256), processing only new/changed files
- **Extracts multi-dimensional metadata** — visual embeddings (CLIP via ONNX Runtime), video frames, EXIF, media properties (ffprobe), and derived tags
- **Enables hybrid search** — combining ANN vector similarity with scalar metadata filters in a single query
- **Supports model migration** — when a better embedding model becomes available, re-embed incrementally without downtime
- **Runs fully offline** on Apple Silicon (MPS/CoreML) with no cloud dependencies

## Scope

### In scope (v1)
- File scanning with content-hash-based change detection
- Persistent job queue for multi-stage processing (extract → embed → index)
- CLIP embedding via ONNX Runtime (CoreML backend on Apple Silicon)
- Video frame extraction via ffmpeg
- Image/video metadata extraction (EXIF, ffprobe)
- LanceDB-backed catalog with hybrid search (ANN + scalar filters)
- HTTP search API with web UI
- CLI for ingest control, search, and catalog management
- Embedding model versioning and incremental re-embedding
- **Duplicate detection** — exact (SHA256), near-visual (CLIP similarity), and cross-video (shared footage). Surfaced via CLI report and UI.
- **Video transcoding** — optional normalization of videos to a configurable max standard (e.g., 1080p H.265 CRF 23) during ingest, with original preservation policy (keep/archive/replace). Leverages the existing ffmpeg dependency and single-read-from-NAS design.

### Out of scope (v1)
- Multi-user / auth
- Cloud deployment or remote storage
- Audio analysis / transcription
- OCR / text extraction from frames
- Real-time streaming ingest
- Mobile/native UI

## Constraints

| Constraint | Detail |
|---|---|
| Platform | macOS / Apple Silicon primary. Linux secondary. |
| Hardware | Mac Mini M4, 16 GB unified RAM, 10-core GPU, 16-core Neural Engine |
| Library size | ~1M items, ~4TB, growing |
| Storage | Media on NAS (Ethernet to Orbi router, Wi-Fi backhaul to mesh satellite, Ethernet to Mac Mini). ~200-400 Mbps effective throughput to media files. |
| Network | Fully offline after initial model download. No eGPU option (Apple Silicon does not support external GPUs). |
| Storage engine | LanceDB (local, file-based, Rust-native) |
| Embedding model | CLIP ViT-H-14 (1024-dim), exported to ONNX |
| Language | Rust (with `ort`, `ffmpeg-next`, `lance` crates) |

## Success Criteria

1. Full library scan completes in < 24 hours on first run
2. Incremental re-scan of unchanged library completes in < 5 minutes
3. Search latency < 200ms for hybrid queries at 1M vectors
4. Re-embedding 1M items with a new model completes in < 48 hours
5. Memory usage stays under 4GB during steady-state ingest
6. Single binary, no external services required
