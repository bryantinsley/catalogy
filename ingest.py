#!/usr/bin/env python3
"""Generate CLIP embeddings for images and video frames, store in LanceDB."""

import os
os.environ["HF_HUB_OFFLINE"] = "1"
os.environ["TRANSFORMERS_OFFLINE"] = "1"
os.environ["HF_HUB_DISABLE_TELEMETRY"] = "1"
os.environ["NO_PROXY"] = "*"
os.environ["http_proxy"] = ""
os.environ["https_proxy"] = ""

import argparse
import gc
from pathlib import Path

import lancedb
import numpy as np
import open_clip
import pyarrow as pa
import torch
from PIL import Image
from tqdm import tqdm

IMAGE_EXTENSIONS = {".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp", ".tiff", ".tif"}
VIDEO_EXTENSIONS = {".mp4", ".mov", ".avi", ".mkv", ".webm", ".m4v", ".flv"}

# Batch size tuned for ~6GB memory cap on Apple Silicon
BATCH_SIZE = 4

MODEL_NAME = "ViT-H-14"
PRETRAINED = "laion2b_s32b_b79k"


def get_device() -> str:
    """Select the best available device (MPS for Apple Silicon, else CPU)."""
    if torch.backends.mps.is_available():
        return "mps"
    return "cpu"


def load_model(device: str):
    """Load the CLIP model and preprocessing pipeline."""
    model, _, preprocess = open_clip.create_model_and_transforms(
        MODEL_NAME, pretrained=PRETRAINED
    )
    model = model.to(device)
    model.eval()
    tokenizer = open_clip.get_tokenizer(MODEL_NAME)
    return model, preprocess, tokenizer


def collect_media(media_dir: Path) -> list[dict]:
    """Gather all images (direct) and extracted frames with metadata."""
    items = []

    # Original images
    for p in sorted(media_dir.iterdir()):
        if p.is_file() and p.suffix.lower() in IMAGE_EXTENSIONS:
            items.append({
                "path": str(p),
                "filename": p.name,
                "source_video": "",
                "timestamp_sec": -1.0,
                "media_type": "image",
            })

    # Extracted video frames
    frames_dir = media_dir / "frames"
    if frames_dir.exists():
        for p in sorted(frames_dir.iterdir()):
            if p.is_file() and p.suffix.lower() in IMAGE_EXTENSIONS:
                # Parse timestamp from filename: {stem}_f{idx}_{ts}s.jpg
                parts = p.stem.rsplit("_", 2)
                ts = -1.0
                source = ""
                if len(parts) >= 3:
                    try:
                        ts = float(parts[-1].rstrip("s"))
                    except ValueError:
                        pass
                    # Reconstruct source video name (best effort)
                    source = parts[0]

                items.append({
                    "path": str(p),
                    "filename": p.name,
                    "source_video": source,
                    "timestamp_sec": ts,
                    "media_type": "video_frame",
                })

    return items


def embed_images_batched(
    items: list[dict], model, preprocess, device: str
) -> np.ndarray:
    """Generate CLIP image embeddings in batches to control memory."""
    all_embeddings = []

    for start in tqdm(range(0, len(items), BATCH_SIZE), desc="Embedding"):
        batch_items = items[start : start + BATCH_SIZE]
        images = []
        for item in batch_items:
            try:
                img = Image.open(item["path"]).convert("RGB")
                images.append(preprocess(img))
            except Exception as e:
                print(f"  [SKIP] {item['filename']}: {e}")
                images.append(preprocess(Image.new("RGB", (224, 224))))

        batch_tensor = torch.stack(images).to(device)
        with torch.no_grad():
            features = model.encode_image(batch_tensor)
            features = features / features.norm(dim=-1, keepdim=True)
            all_embeddings.append(features.cpu().numpy())

        del batch_tensor, features
        if device == "mps":
            torch.mps.empty_cache()
        gc.collect()

    return np.concatenate(all_embeddings, axis=0)


def build_table(items: list[dict], embeddings: np.ndarray, db_path: str):
    """Create or overwrite a LanceDB table with the embedded media."""
    db = lancedb.connect(db_path)

    records = []
    for item, vec in zip(items, embeddings):
        records.append({
            "vector": vec.tolist(),
            "path": item["path"],
            "filename": item["filename"],
            "source_video": item["source_video"],
            "timestamp_sec": item["timestamp_sec"],
            "media_type": item["media_type"],
        })

    try:
        db.drop_table("media")
    except Exception:
        pass

    table = db.create_table("media", data=records)
    print(f"Stored {len(records)} records in LanceDB at {db_path}")
    return table


def main():
    parser = argparse.ArgumentParser(description="Ingest media into LanceDB")
    parser.add_argument("--media-dir", type=str, default=os.path.expanduser("~/media_test"))
    parser.add_argument("--db-path", type=str, default="./lancedb_data")
    args = parser.parse_args()

    media_dir = Path(args.media_dir)
    device = get_device()
    print(f"Device: {device}")

    print("Loading CLIP model...")
    model, preprocess, tokenizer = load_model(device)

    print("Collecting media files...")
    items = collect_media(media_dir)
    if not items:
        print("No media found. Run extract_frames.py first if you have videos.")
        return

    print(f"Found {len(items)} items ({sum(1 for i in items if i['media_type']=='image')} images, "
          f"{sum(1 for i in items if i['media_type']=='video_frame')} frames)")

    print("Generating embeddings...")
    embeddings = embed_images_batched(items, model, preprocess, device)

    print("Writing to LanceDB...")
    build_table(items, embeddings, args.db_path)
    print("Done.")


if __name__ == "__main__":
    main()
