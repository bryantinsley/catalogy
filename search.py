#!/usr/bin/env python3
"""CLI semantic search over media assets stored in LanceDB."""

import os
os.environ["HF_HUB_OFFLINE"] = "1"
os.environ["TRANSFORMERS_OFFLINE"] = "1"
os.environ["HF_HUB_DISABLE_TELEMETRY"] = "1"
os.environ["NO_PROXY"] = "*"
os.environ["http_proxy"] = ""
os.environ["https_proxy"] = ""

import argparse

import lancedb
import open_clip
import torch


MODEL_NAME = "ViT-H-14"
PRETRAINED = "laion2b_s32b_b79k"


def get_device() -> str:
    if torch.backends.mps.is_available():
        return "mps"
    return "cpu"


def embed_text(query: str, model, tokenizer, device: str):
    """Encode a text query into a CLIP vector."""
    tokens = tokenizer([query]).to(device)
    with torch.no_grad():
        features = model.encode_text(tokens)
        features = features / features.norm(dim=-1, keepdim=True)
    return features.cpu().numpy()[0]


def search(query: str, db_path: str, top_k: int = 5):
    """Run a semantic search and return results."""
    device = get_device()

    model, _, _ = open_clip.create_model_and_transforms(MODEL_NAME, pretrained=PRETRAINED)
    model = model.to(device).eval()
    tokenizer = open_clip.get_tokenizer(MODEL_NAME)

    query_vec = embed_text(query, model, tokenizer, device)

    db = lancedb.connect(db_path)
    table = db.open_table("media")
    results = table.search(query_vec).metric("cosine").limit(top_k).to_arrow()

    return results


def main():
    parser = argparse.ArgumentParser(description="Semantic media search")
    parser.add_argument("query", type=str, help="Text query (e.g. 'Chinese countryside')")
    parser.add_argument("--top-k", type=int, default=5, help="Number of results to return")
    parser.add_argument("--db-path", type=str, default="./lancedb_data")
    args = parser.parse_args()

    print(f"Searching for: \"{args.query}\"\n")
    results = search(args.query, args.db_path, args.top_k)

    print(f"{'Rank':<6}{'Score':<10}{'Type':<14}{'Filename'}")
    print("-" * 70)
    for i in range(results.num_rows):
        distance = results.column("_distance")[i].as_py()
        score = 1 - distance
        media_type = results.column("media_type")[i].as_py()
        filename = results.column("filename")[i].as_py()
        extra = ""
        if media_type == "video_frame":
            ts = results.column("timestamp_sec")[i].as_py()
            src = results.column("source_video")[i].as_py()
            if ts >= 0:
                extra = f"  (@ {ts:.0f}s in {src})"
        print(f"{i+1:<6}{score:<10.4f}{media_type:<14}{filename}{extra}")


if __name__ == "__main__":
    main()
