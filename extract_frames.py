#!/usr/bin/env python3
"""Extract 1 frame every 30 seconds from video files using OpenCV."""

import argparse
import os
from pathlib import Path

import cv2
from tqdm import tqdm

VIDEO_EXTENSIONS = {".mp4", ".mov", ".avi", ".mkv", ".webm", ".m4v", ".flv"}
FRAME_INTERVAL_SEC = 30


def extract_frames(video_path: Path, output_dir: Path) -> list[dict]:
    """Extract frames from a single video at the configured interval.

    Returns list of dicts with frame metadata.
    """
    cap = cv2.VideoCapture(str(video_path))
    if not cap.isOpened():
        print(f"  [SKIP] Cannot open: {video_path.name}")
        return []

    fps = cap.get(cv2.CAP_PROP_FPS)
    total_frames = int(cap.get(cv2.CAP_PROP_FRAME_COUNT))
    if fps <= 0 or total_frames <= 0:
        print(f"  [SKIP] Invalid metadata: {video_path.name}")
        cap.release()
        return []

    duration_sec = total_frames / fps
    frame_interval = int(fps * FRAME_INTERVAL_SEC)
    stem = video_path.stem

    results = []
    frame_idx = 0
    extracted = 0

    while frame_idx < total_frames:
        cap.set(cv2.CAP_PROP_POS_FRAMES, frame_idx)
        ret, frame = cap.read()
        if not ret:
            break

        timestamp_sec = frame_idx / fps
        out_name = f"{stem}_f{extracted:04d}_{int(timestamp_sec)}s.jpg"
        out_path = output_dir / out_name
        cv2.imwrite(str(out_path), frame, [cv2.IMWRITE_JPEG_QUALITY, 85])

        results.append({
            "path": str(out_path),
            "source_video": str(video_path),
            "timestamp_sec": round(timestamp_sec, 2),
            "media_type": "video_frame",
        })
        extracted += 1
        frame_idx += frame_interval

    cap.release()
    return results


def main():
    parser = argparse.ArgumentParser(description="Extract frames from videos")
    parser.add_argument("--media-dir", type=str, default=os.path.expanduser("~/media_test"),
                        help="Directory containing video files")
    parser.add_argument("--output-dir", type=str, default=None,
                        help="Output directory for frames (default: <media-dir>/frames)")
    args = parser.parse_args()

    media_dir = Path(args.media_dir)
    output_dir = Path(args.output_dir) if args.output_dir else media_dir / "frames"
    output_dir.mkdir(parents=True, exist_ok=True)

    videos = sorted(
        p for p in media_dir.iterdir()
        if p.is_file() and p.suffix.lower() in VIDEO_EXTENSIONS
    )

    if not videos:
        print(f"No videos found in {media_dir}")
        return

    print(f"Found {len(videos)} videos. Extracting frames (1 per {FRAME_INTERVAL_SEC}s)...")
    all_results = []
    for video in tqdm(videos, desc="Videos"):
        results = extract_frames(video, output_dir)
        all_results.extend(results)

    print(f"\nDone. Extracted {len(all_results)} frames to {output_dir}")


if __name__ == "__main__":
    main()
