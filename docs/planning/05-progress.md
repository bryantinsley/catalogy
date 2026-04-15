# Catalogy — Progress Tracker

> Auto-generated 2026-04-15. Update this at session end.

## Phase Status

| Phase | Branch | Status | Summary |
|-------|--------|--------|---------|
| 0 — Skeleton & CI | main | Done | Workspace, 9 crates, clap CLI, GitHub Actions |
| 1 — Scanner + Queue | main | Done | SHA256 change detection, SQLite state DB, job queue |
| 2 — Metadata | main | Done | EXIF (images), ffprobe (video) |
| 3 — Frame Extraction | main | Done | Adaptive scene-change frames, thumbnail gen |
| 4 — Embedding | main | Done | CLIP ViT-H-14 ONNX, batch inference, LanceDB catalog |
| 5 — Search | main | Done | Hybrid search, CLI + HTTP API + basic web UI |
| 6 — Re-embedding | main | Done | Model registry, incremental re-embed, index rebuild |
| 7 — Dedup | main | Done | Exact (SHA256), visual (cosine > 0.92), cross-video |
| 8 — Transcoding | main | Done | H.265, adaptive quality presets, ffmpeg |
| 9 — Hardening | main | Done | Graceful shutdown, progress bars, tracing, memory opt |
| 10 — Setup | main | Done | `catalogy setup` + `catalogy doctor` diagnostics |
| 11 — Web UI | main | Done | Dashboard, browse, status, SSE progress, search UI |
| 12 — UI Polish | main | Done | Inline scan form, toast notifications, auto-refresh |
| 13 — Ingest Fixes | main | Done | Thumbnail path fix, embed session reuse |

## Notable Bug Fixes

- **State DB path None on fresh start** (54b6821) — stats returned 0 after scan because DB path wasn't set until restart. Fixed: always compute path, StateDb::open creates on demand.
- **ONNX Runtime silent hang** (e90f95e) — `load-dynamic` tried dlopen on missing dylib. Fixed: switched to `download-binaries` for static linking (142MB binary).
- **Catalog panic in async** (14b3ebb) — Catalog methods panicked inside axum handlers. Fixed async/sync boundary.
- **Thumbnail dir literal `~`** (3438c0e) — hardcoded `~/.local/share/...` created a literal tilde directory. Fixed: `data_dir.join("thumbnails")`.
- **Embed session double-load** (3438c0e) — web ingest loaded 1.3GB text.onnx again instead of reusing SearchEngine's session. Fixed: added `embed_session()` getter.

## Local Setup (not in repo)

To run from scratch on a new machine:

```bash
# 1. Install system deps
brew install ffmpeg

# 2. Export CLIP models (one-time)
catalogy setup          # downloads/exports ONNX models + tokenizer

# 3. Run
catalogy serve --host 0.0.0.0 --port 8080
```

Models are stored in `~/.local/share/catalogy/models/` (not committed).

## What's Left

Nothing is actively in-progress. Future ideas from the original spec:
- Performance tuning for full 4TB library
- Continuous file watching (inotify/FSEvents)
- Library analytics dashboard
- Catalog export/backup
