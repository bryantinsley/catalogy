# catalogy

A fully local, offline semantic media search engine for images and video frames. Runs entirely on Apple Silicon (MPS GPU) with no external API calls.

## What it does

Given a folder of images and videos, catalogy:

1. **Extracts frames** from videos (1 frame every 30 seconds) using OpenCV
2. **Generates CLIP embeddings** for all images and extracted frames using OpenAI's CLIP model via `open_clip`
3. **Stores vectors + metadata** in a local LanceDB instance
4. **Searches semantically** — describe what you're looking for in plain text and get back the most visually similar media

## Offline by design

After the initial model weight download, catalogy never makes network requests. All scripts set `HF_HUB_OFFLINE=1`, `TRANSFORMERS_OFFLINE=1`, and null out proxy environment variables before any library imports. The CLIP model runs locally on your GPU — no OpenAI, Vertex, or any other cloud API involved.

## Model

The project uses **ViT-H-14** (`laion2b_s32b_b79k` weights) — a large CLIP model with 14px patch size and 1024-dimensional embeddings.

We started with ViT-B-32 (the smallest/fastest CLIP variant), but its 32px patch size was too coarse for fine-grained visual discrimination. It struggled to distinguish attributes like color (e.g., "yellow flower" vs "red flower" returned results within 0.02 similarity of each other). ViT-H-14's finer patch resolution and larger embedding space produce much better separation for attribute-level queries.

Other options available via `open_clip` if you want to experiment:

| Model | Embedding dim | VRAM | Quality |
|---|---|---|---|
| ViT-B-32 | 512 | ~0.5GB | Good — fast but coarse |
| ViT-B-16 | 512 | ~0.6GB | Better — good sweet spot |
| ViT-L-14 | 768 | ~1.5GB | Very good |
| **ViT-H-14** (current) | 1024 | ~2.5GB | Excellent |
| ViT-bigG-14 | 1280 | ~5GB | State of the art |

To change models, update `MODEL_NAME` and `PRETRAINED` in `ingest.py`, `search.py`, and `web.py`, then re-run ingestion.

## Supported formats

**Video:** `.mp4`, `.mov`, `.avi`, `.mkv`, `.webm`, `.m4v`, `.flv`

**Image:** `.jpg`, `.jpeg`, `.png`, `.gif`, `.webp`, `.bmp`, `.tiff`, `.tif`

Anything OpenCV can decode will work. Add extensions to the `VIDEO_EXTENSIONS` / `IMAGE_EXTENSIONS` sets if you need more.

## Setup

```bash
# Requires Python 3.10+ and Homebrew python recommended
python3 -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
```

The first run of any script that loads the CLIP model will download the weights (~2.5GB for ViT-H-14). This is the only time internet access is needed.

## Usage

Set your paths:

```bash
export MEDIA_PATH=~/my_media
export DB_PATH=~/my_db
```

### 1. Extract frames from videos

```bash
python extract_frames.py --media-dir "$MEDIA_PATH"
```

Writes frames to `$MEDIA_PATH/frames/` as JPEGs named `{video_stem}_f{index}_{timestamp}s.jpg`.

### 2. Ingest everything into the vector database

```bash
python ingest.py --media-dir "$MEDIA_PATH" --db-path "$DB_PATH"
```

Generates CLIP embeddings on the MPS GPU in batches of 4 (tuned to stay under 6GB total memory). Stores vectors plus metadata (filename, source video, timestamp, media type) in LanceDB.

### 3. Search via CLI

```bash
python search.py "Chinese countryside" --db-path "$DB_PATH" --top-k 5
```

Returns ranked results with similarity scores:

```
Rank  Score     Type          Filename
----------------------------------------------------------------------
1     0.2147    image         chinese_countryside_v2.jpg
2     0.2105    image         chinese_countryside_v1.jpg
3     0.2057    image         japanese_garden_v2.jpg
```

### 4. Search via web UI

```bash
python web.py --db-path "$DB_PATH" --media-dir "$MEDIA_PATH" --port 8765
```

Opens a dark-themed web interface at `http://localhost:8765` with a search box and inline 200px-wide thumbnails for each result.

## File overview

| File | Purpose |
|---|---|
| `extract_frames.py` | OpenCV frame extraction (1 per 30s) |
| `ingest.py` | CLIP embedding generation + LanceDB storage |
| `search.py` | CLI semantic search |
| `web.py` | Web UI with thumbnail results |
| `generate_samples.py` | Synthetic test data generator (not needed for real use) |

## Notes

- Filenames are stored as metadata but do **not** influence the embedding. Search is purely visual — a file named `beach.jpg` containing a cat will match "cat" queries.
- Re-ingestion is required when changing models, since embedding dimensions differ.
- The batch size in `ingest.py` is set to 4 for ViT-H-14 to keep memory under 6GB. Increase it if using a smaller model.
