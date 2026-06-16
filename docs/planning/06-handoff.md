# Catalogy — Handoff: Finish the Implementation

> Written 2026-06-15. Goal: take catalogy from "feature-complete on paper" to
> "verified end-to-end and trustworthy on the real library." The phase table in
> [05-progress.md](05-progress.md) marks every phase "Done", but several pieces
> are unverified, and test coverage of the surfaces a user actually touches (HTTP
> API, web UI, full pipeline) is thin. This doc is the plan to close that.

## Current state (as of this handoff)

- `main` (= `origin/main`, commit `a124153`) is the integrated trunk. It now
  contains the GPU/CUDA embedding path, true-cosine scoring, dynamic-batch
  visual model + batched frame embedding, per-media-item search result
  collapsing, and the Phase 4.7 ANN index. Full workspace test suite is green.
- The live systemd service (`--user`, port 18080) runs this build, GPU-resident
  on the RTX 3090 (~5.4 GB).
- **`devel` branch** holds ~1400 lines of *uncommitted-then-preserved* WIP
  (commit `5a1daae`, not pushed): a config-management API, a settings web-UI
  page, and a transcode trigger, plus a heavy `catalog.rs` rewrite. It is
  **unverified** and carries a known SIGTERM-deadlock concern. See "Decision: the
  devel WIP" below before integrating any of it.
- Remote branches `phase10/11/12/13` are fully merged into `main` and safe to
  delete (`git push origin --delete <branch>`).

## Priority 1 — Verify what we claim is done

The biggest risk is "Done" features that have never been exercised together.

1. **Full-pipeline integration test** (Rust, `tests/` at workspace root or a
   `catalogy` bin integration test). Scan a small fixture dir of real
   images + a short video → ingest all stages → assert catalog row counts
   (image rows, `video` + `video_frame` rows), then run a text search and assert
   a known item ranks #1. Gate on `CATALOGY_MODEL_DIR` like the embed tests.
2. **ANN index at real scale.** The IVF-PQ path is unit-tested on 600 synthetic
   rows but never on a real ≥1000-row catalog. Do a real-library ingest (see
   Priority 4), confirm the index builds automatically, and spot-check that
   search latency and `refine_factor` scores look right.
3. **reembed worker video gap (Phase 6).** `run_reembed_worker` still calls
   `embed_image` on the raw file path — broken for videos exactly like the embed
   worker was before Task 4.6. Mirror the `embed_video` branch into the re-embed
   worker, then add a test.

## Priority 2 — Tests for the surfaces users touch

### HTTP API integration tests (`catalogy-server`)
Spin up the axum app against a temp catalog + state DB (no real model needed for
most — inject a stub or a tiny model), then drive it with `reqwest` or axum's
`oneshot`:
- `POST /api/search` — JSON in/out, filters, `timestamp_ms` on video results.
- `GET /api/media/:id` — **Range header support** (partial-content 206; this is
  what makes video seeking work and is easy to regress).
- `GET /api/thumb/:id` — content-type, 404 on missing.
- `GET /api/stats`, `/api/files`, `/api/browse`, `/api/progress`.
- `POST /api/scan`, `POST /api/ingest` — kick off work, poll `/api/progress`.
Assert status codes and shapes; these have zero coverage today.

### Web UI end-to-end tests
The UI is vanilla JS embedded via `rust-embed`. Use the **chrome-devtools MCP**
(already available in this environment) or a headless browser driver:
- Serve the binary on a test port against a seeded catalog.
- Search box → results grid renders thumbnails → click a result expands it →
  a video result plays (exercises the Range endpoint).
- Filter controls (media type, date) change results.
- Scan form submits and the progress indicator updates (SSE `/api/progress`).
Keep these out of `cargo test` (they need a browser); add a `just`/script target
like `scripts/e2e.sh` and document running it. The `verify` skill is a good
manual companion.

### Unit-test gaps worth filling
- `media/:id` Range parsing (byte ranges, open-ended, invalid).
- Thumbnail generation (size cap, format) — Phase 3.3 has no direct test.
- Transcode decision engine (Phase 8.1) thresholds.

## Priority 3 — Search quality refinements (improve on contact with reality)
These are known limitations, not bugs — tackle once the real library exposes
them (the user explicitly called these out):
- **Intra-video frame collapsing.** Today a video collapses to a *single*
  result anchored to its best frame. For long videos with several distinct
  relevant moments, we likely want *multiple* results per video — collapse only
  *temporally adjacent / near-duplicate* frames, keep distinct scenes separate.
  This interacts with the dedup threshold and `frame_timestamp_ms`.
- **Search pagination.** `SearchEngine::search` uses a fixed `limit` and a fixed
  pre-fetch `fetch_limit = max(limit*8, 80)`. On a large library where one
  video's frames dominate the top rows, collapse can return fewer than `limit`
  distinct items. Add real pagination (offset/cursor) and make the pre-fetch
  adaptive, or de-duplicate at the catalog-query level.
- **CLI/help nit:** `search --type` is surfaced as `--media-type` in help text;
  reconcile.

## ~~Decision: the devel WIP~~ — DONE (integrated 2026-06-16)
The useful slice of the devel WIP (config-management API, settings UI,
transcode trigger, config-driven scan/ingest) was re-applied onto `main` and is
deployed: see commits `dca2c3f` + `9561256`. The `catalog.rs` rewrite was
**deliberately discarded** (it predated `main`'s `BackgroundRuntime` async-drop
fix + ANN index + refine, and carried the SIGTERM-deadlock risk). A
`Config::to_file` ENOENT bug found during integration was fixed (creates the
config dir). Covered by `tests/config_api.rs` + a `Config::to_file` unit test.
The `devel` branch (`5a1daae`, unpushed) is now superseded — its only unique
remaining content is the rejected rewrite; safe to delete.

**Still owed here:** a shutdown test (Phase 9.1: SIGTERM during `serve` exits
cleanly) to lock out the deadlock class for good, and web-UI E2E coverage of the
new Settings page.

## Priority 4 — The real-library run (the payoff)
Point ingest at a real library directory and let it run on the GPU. This both
validates throughput at scale and is where the search-quality refinements above
will earn their keep. Watch: peak memory (Phase 9.4 target < 4 GB), throughput,
and that the ANN index builds automatically past `MIN_INDEX_ROWS` (1000).

## Ops notes (machine-local, not in the repo)
- **GPU deploy is not just `cargo install`.** Build `--features cuda`, then copy
  the ORT provider libs next to the binary: `cp -L target/release/*.so*
  ~/.cargo/bin/`. systemd env (`~/.config/catalogy/catalogy.env`) sets
  `CATALOGY_EXECUTION_PROVIDER=cuda`, `CATALOGY_CUDA_DEVICE=0`,
  `CUDA_VISIBLE_DEVICES=<3090 UUID>`, `CATALOGY_EMBED_BATCH_SIZE=16`, and
  `LD_LIBRARY_PATH` to CUDA 12.8 libs + cuDNN. Details in
  [progress.md](progress.md).
- **cuDNN is borrowed** from ollama's bundle via `LD_LIBRARY_PATH`. A proper
  system cuDNN 9 install would remove that fragile dependency.

## Minor cleanup
- Two progress docs coexist: `05-progress.md` (canonical tracker) and
  `progress.md` (detailed session journal). Consider folding the journal into
  the tracker or clearly delineating their roles.
- `catalogy ingest --workers` is parsed but not threaded into `run_ingest`.
