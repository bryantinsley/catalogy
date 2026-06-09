# catalogy

A local-first, **strictly-offline** semantic media search engine for images and
video frames. Point it at a folder of media, describe what you're looking for in
plain text, and get back the most visually similar images and video frames — no
cloud APIs, no data leaving the machine.

Under the hood: CLIP embeddings (ViT-H-14, 1024-dim) stored in LanceDB and searched
by cosine/ANN similarity. It runs as a CLI and an HTTP server with a small web UI.

> **Implementation note.** catalogy is a Rust workspace. An earlier Python prototype
> has been removed; the only remaining Python is `scripts/export_clip.py`, a one-time
> CLIP→ONNX exporter. Working on the code? Start with **[AGENTS.md](AGENTS.md)** — the
> authoritative primer on architecture, conventions, and gotchas.

## Offline by design

After the CLIP weights are exported once (the only step that touches the network),
catalogy never makes another outbound request. The runtime is pure Rust + local ONNX
inference — no OpenAI, no Vertex, no hosted vector DB. The model export sets the usual
`HF_HUB_OFFLINE` / proxy guards.

## How it works

1. **Scan** a directory — hash files, detect changes, queue work.
2. **Extract** frames from videos (FFmpeg, adaptive selection) + thumbnails.
3. **Embed** every image and frame with CLIP (ONNX via `ort`) → 1024-dim vectors.
4. **Store** vectors + rich metadata (EXIF, codec, dimensions, timestamps) in LanceDB.
5. **Search** semantically from the CLI or the web UI.

## Install

Requires a stable Rust toolchain and FFmpeg/ffprobe on `PATH`.

```sh
cargo build --release
# binary at ./target/release/catalogy  (or `cargo install --path .`)
```

### Model weights (one-time)

The server needs `visual.onnx`, `text.onnx`, and `tokenizer.json`. Export them from
open_clip with the bundled script (downloads weights on first run, then offline):

```sh
python3 -m venv scripts/.venv && source scripts/.venv/bin/activate
pip install -r scripts/requirements.txt
python scripts/export_clip.py --output-dir ~/.local/share/catalogy/models
```

`catalogy setup` checks dependencies and can run this for you. Point elsewhere with
`CATALOGY_MODEL_DIR`.

## Usage

```sh
catalogy scan --path ~/Media        # index a folder, queue jobs
catalogy ingest                     # run workers: frames -> metadata -> embeddings
catalogy search "rainy city street" # semantic search (--limit, --type, --after)
catalogy status                     # queue + catalog stats
catalogy serve                      # HTTP API + web UI at http://localhost:18080
```

Other subcommands: `dedup` (exact/visual/cross-video duplicate detection), `reembed`
(swap embedding models, rebuild the ANN index), `transcode` (video transcode policy),
`config --init`, `setup`.

Default data lives under `~/.local/share/catalogy/` (`catalog.lance`, `state.db`,
`models/`).

## Running the server

```sh
catalogy serve --port 18080
```

- Default port is **18080** (8080 is avoided — it collides with common dev/proxy
  setups). Override with `--port`.
- If the port is taken, catalogy exits with a clear `port NNNN is already in use`
  message — no panic.
- It binds `0.0.0.0` (reachable on your LAN). Shuts down gracefully on Ctrl+C and on
  `SIGTERM`, so it stops cleanly under a process manager.

### As a service (systemd)

`packaging/systemd/` ships a unit template, an env example, and a README covering
both system-wide and rootless `--user` installs (graceful stop, restart-on-failure,
boot start). See [`packaging/systemd/README.md`](packaging/systemd/README.md).

## The model

**ViT-H-14** (`laion2b_s32b_b79k`), 1024-dim embeddings — chosen over smaller CLIP
variants for attribute-level discrimination (e.g. "yellow flower" vs "red flower",
which coarser ViT-B-32 conflates). Other variants are exportable via `open_clip`;
changing the model means re-embedding the catalog (different dimensions):

| Model | Dim | VRAM | Quality |
|---|---|---|---|
| ViT-B-32 | 512 | ~0.5 GB | fast but coarse |
| ViT-B-16 | 512 | ~0.6 GB | good sweet spot |
| ViT-L-14 | 768 | ~1.5 GB | very good |
| **ViT-H-14** (current) | 1024 | ~2.5 GB | excellent |
| ViT-bigG-14 | 1280 | ~5 GB | state of the art |

## Supported formats

**Video:** `.mp4`, `.mov`, `.avi`, `.mkv`, `.webm`, `.m4v`, `.flv`
**Image:** `.jpg`, `.jpeg`, `.png`, `.gif`, `.webp`, `.bmp`, `.tiff`, `.tif`

## Notes

- Search is purely visual — filenames are stored as metadata but don't influence
  embeddings. A file named `beach.jpg` containing a cat matches "cat".
- Architecture, crate map, build/test details, and the runtime gotchas live in
  **[AGENTS.md](AGENTS.md)**.
