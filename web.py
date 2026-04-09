#!/usr/bin/env python3
"""Minimal web UI for semantic media search."""

import os
os.environ["HF_HUB_OFFLINE"] = "1"
os.environ["TRANSFORMERS_OFFLINE"] = "1"
os.environ["HF_HUB_DISABLE_TELEMETRY"] = "1"
os.environ["NO_PROXY"] = "*"
os.environ["http_proxy"] = ""
os.environ["https_proxy"] = ""

import argparse
from http.server import HTTPServer, BaseHTTPRequestHandler
from pathlib import Path
from urllib.parse import parse_qs, urlparse, quote, unquote

import lancedb
import open_clip
import torch

MODEL_NAME = "ViT-H-14"
PRETRAINED = "laion2b_s32b_b79k"

# Globals set at startup
DB_PATH = ""
MEDIA_PATH = ""
MODEL = None
TOKENIZER = None
DEVICE = ""


def init_model():
    global MODEL, TOKENIZER, DEVICE
    DEVICE = "mps" if torch.backends.mps.is_available() else "cpu"
    print(f"Loading CLIP model on {DEVICE}...")
    model, _, _ = open_clip.create_model_and_transforms(MODEL_NAME, pretrained=PRETRAINED)
    MODEL = model.to(DEVICE).eval()
    TOKENIZER = open_clip.get_tokenizer(MODEL_NAME)
    print("Model ready.")


def embed_text(query: str):
    tokens = TOKENIZER([query]).to(DEVICE)
    with torch.no_grad():
        features = MODEL.encode_text(tokens)
        features = features / features.norm(dim=-1, keepdim=True)
    return features.cpu().numpy()[0]


def do_search(query: str, top_k: int = 20):
    vec = embed_text(query)
    db = lancedb.connect(DB_PATH)
    table = db.open_table("media")
    results = table.search(vec).metric("cosine").limit(top_k).to_arrow()
    rows = []
    for i in range(results.num_rows):
        rows.append({
            "score": 1 - results.column("_distance")[i].as_py(),
            "filename": results.column("filename")[i].as_py(),
            "path": results.column("path")[i].as_py(),
            "media_type": results.column("media_type")[i].as_py(),
            "timestamp_sec": results.column("timestamp_sec")[i].as_py(),
            "source_video": results.column("source_video")[i].as_py(),
        })
    return rows


HTML_PAGE = """<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>videlib search</title>
<style>
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body {
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
    background: #0e0e10;
    color: #e0e0e0;
    min-height: 100vh;
  }
  .search-wrap {
    max-width: 640px;
    margin: 0 auto;
    padding: 80px 20px 40px;
    text-align: center;
  }
  .search-wrap h1 {
    font-size: 1.6rem;
    font-weight: 600;
    margin-bottom: 24px;
    color: #f0f0f0;
    letter-spacing: -0.02em;
  }
  form { display: flex; gap: 8px; }
  input[type="text"] {
    flex: 1;
    padding: 12px 16px;
    border-radius: 8px;
    border: 1px solid #2a2a2e;
    background: #1a1a1e;
    color: #f0f0f0;
    font-size: 1rem;
    outline: none;
    transition: border-color 0.15s;
  }
  input[type="text"]:focus { border-color: #5c7cfa; }
  input[type="text"]::placeholder { color: #666; }
  button {
    padding: 12px 24px;
    border-radius: 8px;
    border: none;
    background: #5c7cfa;
    color: #fff;
    font-size: 1rem;
    font-weight: 500;
    cursor: pointer;
    transition: background 0.15s;
  }
  button:hover { background: #4c6ef5; }
  .results {
    max-width: 900px;
    margin: 0 auto;
    padding: 20px;
  }
  .results-header {
    font-size: 0.85rem;
    color: #888;
    margin-bottom: 16px;
    padding-left: 4px;
  }
  .result-card {
    display: flex;
    gap: 16px;
    align-items: flex-start;
    padding: 14px;
    margin-bottom: 10px;
    background: #1a1a1e;
    border-radius: 10px;
    border: 1px solid #2a2a2e;
    transition: border-color 0.15s;
  }
  .result-card:hover { border-color: #3a3a3e; }
  .result-card img {
    width: 200px;
    height: auto;
    border-radius: 6px;
    flex-shrink: 0;
    background: #111;
  }
  .result-meta {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }
  .result-meta .filename {
    font-weight: 500;
    font-size: 0.95rem;
    word-break: break-all;
  }
  .result-meta .details {
    font-size: 0.82rem;
    color: #888;
  }
  .badge {
    display: inline-block;
    padding: 2px 8px;
    border-radius: 4px;
    font-size: 0.75rem;
    font-weight: 500;
  }
  .badge-image { background: #1e3a2f; color: #6ee7a0; }
  .badge-frame { background: #1e2a3a; color: #6eaae7; }
  .score {
    font-variant-numeric: tabular-nums;
    color: #aaa;
    font-size: 0.82rem;
  }
</style>
</head>
<body>
  <div class="search-wrap">
    <h1>videlib</h1>
    <form method="get" action="/search">
      <input type="text" name="q" placeholder="Describe what you're looking for..." value="QUERY_PLACEHOLDER" autofocus>
      <button type="submit">Search</button>
    </form>
  </div>
  RESULTS_PLACEHOLDER
</body>
</html>"""


def render_results(query: str, rows: list[dict]) -> str:
    if not rows:
        return '<div class="results"><div class="results-header">No results.</div></div>'
    cards = []
    for i, r in enumerate(rows):
        img_url = f"/media?p={quote(r['path'])}"
        badge_cls = "badge-image" if r["media_type"] == "image" else "badge-frame"
        badge_label = r["media_type"].replace("_", " ")
        extra = ""
        if r["media_type"] == "video_frame" and r["timestamp_sec"] >= 0:
            extra = f' &middot; {r["timestamp_sec"]:.0f}s into <em>{r["source_video"]}</em>'
        cards.append(f"""
    <div class="result-card">
      <img src="{img_url}" alt="{r['filename']}" loading="lazy">
      <div class="result-meta">
        <span class="filename">{r['filename']}</span>
        <span class="details">
          <span class="badge {badge_cls}">{badge_label}</span>
          <span class="score">{r['score']:.4f}</span>
          {extra}
        </span>
      </div>
    </div>""")
    header = f'<div class="results-header">{len(rows)} results for &ldquo;{query}&rdquo;</div>'
    return f'<div class="results">{header}{"".join(cards)}</div>'


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        parsed = urlparse(self.path)

        # Serve media files
        if parsed.path == "/media":
            params = parse_qs(parsed.query)
            raw = params.get("p", [""])[0]
            file_path = Path(unquote(raw))
            if file_path.exists() and file_path.is_file():
                ext = file_path.suffix.lower()
                ct = {"jpg": "image/jpeg", "jpeg": "image/jpeg", "png": "image/png",
                      "gif": "image/gif", "webp": "image/webp", "bmp": "image/bmp",
                      "tiff": "image/tiff", "tif": "image/tiff"}.get(ext.lstrip("."), "application/octet-stream")
                self.send_response(200)
                self.send_header("Content-Type", ct)
                self.end_headers()
                self.wfile.write(file_path.read_bytes())
            else:
                self.send_error(404)
            return

        # Search page
        if parsed.path == "/search":
            params = parse_qs(parsed.query)
            query = params.get("q", [""])[0].strip()
            rows = do_search(query) if query else []
            html = HTML_PAGE.replace("QUERY_PLACEHOLDER", query).replace("RESULTS_PLACEHOLDER", render_results(query, rows))
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.end_headers()
            self.wfile.write(html.encode())
            return

        # Home
        html = HTML_PAGE.replace("QUERY_PLACEHOLDER", "").replace("RESULTS_PLACEHOLDER", "")
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.end_headers()
        self.wfile.write(html.encode())

    def log_message(self, format, *args):
        if "/media/" not in str(args[0]):
            super().log_message(format, *args)


def main():
    parser = argparse.ArgumentParser(description="Web UI for semantic media search")
    parser.add_argument("--db-path", type=str, default="./lancedb_data")
    parser.add_argument("--media-dir", type=str, default=os.path.expanduser("~/media_test"))
    parser.add_argument("--port", type=int, default=8765)
    args = parser.parse_args()

    global DB_PATH, MEDIA_PATH
    DB_PATH = args.db_path
    MEDIA_PATH = args.media_dir

    init_model()

    server = HTTPServer(("0.0.0.0", args.port), Handler)
    print(f"\nServing at http://localhost:{args.port}")
    print(f"  DB:    {DB_PATH}")
    print(f"  Media: {MEDIA_PATH}")
    print("  Ctrl+C to stop\n")
    server.serve_forever()


if __name__ == "__main__":
    main()
