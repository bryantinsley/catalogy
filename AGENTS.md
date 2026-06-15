# AGENTS.md — primer for AI assistants & contributors

Authoritative, terse orientation for working on **catalogy**. Read this before
acting. If something here conflicts with older notes (or the model's prior
training about this repo), this file wins. Human-facing usage lives in
[README.md](README.md); the two are meant to be kept in sync — if you change
behavior, update both.

## What this is

A **local-first, strictly-offline semantic media search engine** for images and
video frames. You point it at a folder; it extracts video frames, computes CLIP
embeddings (ViT-H-14, 1024-dim), stores vectors + metadata in LanceDB, and serves
plain-text semantic search over them — CLI and HTTP.

**The project is Rust.** A Python prototype (`ingest.py`/`search.py`/`web.py`/
`extract_frames.py`) used to live at the repo root; it has been **removed**. The
*only* remaining Python is `scripts/export_clip.py`, a one-time tool that exports
the CLIP model to ONNX for the Rust runtime (see [Model weights](#model-weights)).
Do not reintroduce a Python runtime path.

## Stack & layout

- Cargo workspace, **edition 2021**, no toolchain pin (stable Rust).
- Root package `catalogy` (`src/main.rs`) — the CLI binary; wires crates together.
- 12 library crates under `crates/catalogy-*`, roughly in pipeline order:

| Crate | Responsibility |
|---|---|
| `catalogy-core` | Shared types, `CatalogyError`/`Result`, config (`Config::from_file`), `MediaType`, `FileHash` (SHA256), job/stage enums. Everything depends on this. |
| `catalogy-scanner` | Recursive directory scan; SHA256 hashing → `Vec<ScannedFile>`. |
| `catalogy-queue` | SQLite `state.db`: tracks files, detects new/modified/moved/deleted, enqueues `jobs`, registers models. |
| `catalogy-extract` | Video → frames via FFmpeg (adaptive/scene selection); thumbnails. |
| `catalogy-metadata` | Image EXIF + dimensions; video duration/fps/codec/bitrate via `ffprobe`. |
| `catalogy-embed` | ONNX CLIP inference (`ort`). `visual.onnx`/`text.onnx`/`tokenizer.json` → 1024-dim vectors. `CLIP_DIMENSIONS = 1024`. |
| `catalogy-catalog` | LanceDB `catalog.lance` vector store: upsert/search/index. **See runtime gotcha below.** |
| `catalogy-search` | Query parsing (`type:`, `after:` filters) + vector search over the catalog. |
| `catalogy-dedup` | Duplicate detection: exact (hash), visual (cosine), cross-video. |
| `catalogy-transcode` | Transcode-eligibility policy + FFmpeg execution + verification. |
| `catalogy-setup` | Dependency checks (ffmpeg/ffprobe/python3), dir creation, model export via `scripts/export_clip.py`. |
| `catalogy-server` | Axum HTTP API + embedded static frontend. |

## Build / test / run

```sh
cargo build --release            # whole workspace
cargo test                       # full suite; hermetic — no network, no model files,
                                 # no env vars needed (tests use tempfile dirs)
cargo run --release -- <SUBCMD>  # or ./target/release/catalogy <SUBCMD>
```

If you change `serve`/runtime/shutdown code, don't trust unit tests alone — run
the binary and signal it (see [Runtime gotcha](#runtime-gotcha-read-before-touching-serve-or-catalog)).

## CLI subcommands

`scan` (index a dir, enqueue jobs) · `ingest` (run queue workers: frames→metadata→embed;
`--stages` to subset) · `search <query>` (`--limit/--type/--after`) · `status` (queue +
catalog stats) · `dedup` (`--tier exact|visual|cross-video|all`, `--threshold`) ·
`reembed` (register/activate models, rebuild index) · `serve` (`--port`, default **18080**) ·
`config` (`--init`) · `transcode` (`--dry-run`/`--run`) · `setup` (readiness + model export).

Typical flow: `scan --path ~/Media` → `ingest` → `search "sunset"` / `serve`.

## HTTP API (catalogy-server)

`create_router()` mounts (CORS permissive): `POST /api/search`, `GET /api/media/{id}`,
`GET /api/thumb/{id}`, `GET /api/stats`, `GET /api/stats/full`, `GET /api/dedup`,
`GET /api/setup/status`, `GET /api/files`, `POST /api/scan`, `POST /api/ingest`,
`GET /api/progress`, `GET /api/browse`, and `/` + `/{*path}` for the embedded UI.
`AppState { catalog: Arc<Catalog>, search_engine: Option<SearchEngine>, state_db_path,
model_dir, data_dir, progress: Mutex<_> }`. `search_engine` is `None` when model files
are absent — the server still runs, search just returns nothing.

## Conventions & invariants

- **Errors:** return `catalogy_core::Result<T>` (= `Result<T, CatalogyError>`). Add a
  `CatalogyError` variant rather than stringly-typed errors or `unwrap` in library code.
- **Strictly offline:** after weights exist, nothing may hit the network. The Rust
  runtime makes no outbound calls; keep it that way. Only `scripts/export_clip.py`
  ever downloads (the CLIP weights, once).
- **Env vars actually read:** `CATALOGY_MODEL_DIR` (dir holding the ONNX files) and
  `RUST_LOG` (e.g. `RUST_LOG=catalogy=debug`). Port is a flag, not an env var.
- **Default paths** (`src/main.rs`): data dir = `dirs::data_local_dir()/catalogy`
  (`~/.local/share/catalogy` on Linux); `state.db`, `catalog.lance`, and `models/`
  (overridable via `CATALOGY_MODEL_DIR`) live under it; config = `dirs::config_dir()/catalogy/config.toml`.
- **Embeddings:** ViT-H-14, **1024 dims**, float32, stored as Arrow `FixedSizeList`.
  Changing the model/dims means re-embedding the whole catalog (`reembed`).
- **Git:** this repo's owner authors their own commits — **do not add
  `Co-Authored-By: Claude` trailers**. Branch off `main`; commit/push only when asked.

## Runtime gotcha (READ before touching `serve` or `Catalog`)

`Catalog` (`crates/catalogy-catalog/src/catalog.rs`) **owns its own tokio `Runtime`**
so its sync API can drive async LanceDB. That runtime is wrapped in `BackgroundRuntime`,
whose `Drop` calls `shutdown_background()`. This exists because the server's `AppState`
owns the `Catalog`, and dropping a tokio `Runtime` *inside another runtime's `block_on`*
(which happens on `serve` bind-failure and on graceful shutdown) panics with "Cannot drop
a runtime in a context where blocking is not allowed." Do **not** revert to a bare
`Runtime` field, and don't assume `#[tokio::main]` fixes it — `main` is intentionally
sync. `serve` binds, then runs `axum::serve(...).with_graceful_shutdown(wait_for_shutdown())`;
the `SHUTDOWN` flag is set by a `ctrlc` handler built with the `termination` feature so it
fires on **SIGINT and SIGTERM/SIGHUP** (the latter is what `systemctl stop` sends).

Other sharp edges: default port is **18080** (8080 is a very common dev/proxy port and
collided in practice); a busy port now produces a clear "port already in use" error and
exit 1, not a panic. `serve` binds `0.0.0.0` (LAN-reachable), ignoring `config.toml`'s
`host` — tighten to loopback if that matters.

## Model weights

The Rust server needs `visual.onnx`, `text.onnx`, `tokenizer.json` in the model dir.
Generate them once with the only remaining Python:

```sh
pip install -r scripts/requirements.txt
python scripts/export_clip.py --output-dir ~/.local/share/catalogy/models
```

`catalogy setup` can drive this for you (it locates `scripts/export_clip.py`).

## Running as a service

`packaging/systemd/` has a unit template, an env example, and a README covering both
system and `--user` installs. `serve` shuts down gracefully on SIGTERM, so `systemctl
stop` is clean (no SIGKILL).
