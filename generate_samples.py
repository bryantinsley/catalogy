#!/usr/bin/env python3
"""Generate synthetic test media for the semantic search pipeline."""

import os
from pathlib import Path

import cv2
import numpy as np

MEDIA_DIR = Path(os.path.expanduser("~/media_test"))
MEDIA_DIR.mkdir(parents=True, exist_ok=True)

# Color palettes and labels for different scene types
SCENES = {
    # Nature / landscapes
    "mountain_sunrise": {"sky": (255, 180, 100), "ground": (60, 120, 60), "accent": (255, 220, 150)},
    "ocean_beach": {"sky": (235, 200, 140), "ground": (210, 190, 140), "accent": (180, 120, 60)},
    "forest_path": {"sky": (180, 200, 160), "ground": (40, 80, 30), "accent": (80, 140, 50)},
    "desert_dunes": {"sky": (255, 220, 180), "ground": (200, 170, 100), "accent": (220, 190, 120)},
    "lake_reflection": {"sky": (160, 200, 230), "ground": (60, 100, 60), "accent": (100, 160, 200)},
    "snowy_mountains": {"sky": (200, 220, 240), "ground": (240, 245, 250), "accent": (180, 200, 220)},
    "autumn_forest": {"sky": (180, 200, 220), "ground": (80, 60, 30), "accent": (200, 120, 40)},
    "tropical_island": {"sky": (240, 200, 140), "ground": (60, 140, 80), "accent": (40, 180, 200)},
    "rolling_hills": {"sky": (160, 190, 230), "ground": (80, 140, 50), "accent": (100, 160, 60)},
    "waterfall_canyon": {"sky": (140, 170, 200), "ground": (80, 100, 70), "accent": (180, 220, 240)},
    # Urban
    "city_skyline_night": {"sky": (20, 20, 40), "ground": (40, 40, 60), "accent": (255, 200, 50)},
    "busy_street": {"sky": (160, 170, 180), "ground": (100, 100, 110), "accent": (200, 60, 60)},
    "neon_signs": {"sky": (10, 10, 30), "ground": (30, 30, 50), "accent": (255, 50, 200)},
    "downtown_traffic": {"sky": (120, 130, 150), "ground": (80, 80, 90), "accent": (255, 180, 0)},
    "rooftop_view": {"sky": (100, 150, 200), "ground": (140, 140, 150), "accent": (200, 200, 210)},
    # People / activity
    "crowded_market": {"sky": (180, 160, 140), "ground": (140, 100, 70), "accent": (220, 80, 60)},
    "park_picnic": {"sky": (140, 190, 230), "ground": (80, 160, 60), "accent": (220, 200, 100)},
    "concert_stage": {"sky": (20, 10, 40), "ground": (60, 30, 80), "accent": (255, 100, 200)},
    "sports_field": {"sky": (120, 170, 220), "ground": (50, 130, 40), "accent": (255, 255, 255)},
    "children_playground": {"sky": (150, 200, 240), "ground": (100, 160, 80), "accent": (255, 200, 0)},
    # Rural / countryside
    "chinese_countryside": {"sky": (160, 190, 210), "ground": (80, 120, 50), "accent": (160, 140, 100)},
    "rice_terraces": {"sky": (180, 200, 210), "ground": (100, 150, 60), "accent": (120, 170, 80)},
    "farmland_sunset": {"sky": (255, 160, 80), "ground": (120, 100, 40), "accent": (200, 140, 60)},
    "vineyard_hills": {"sky": (140, 180, 220), "ground": (80, 110, 50), "accent": (120, 80, 140)},
    "country_road": {"sky": (150, 190, 230), "ground": (100, 80, 50), "accent": (140, 160, 80)},
    # Architecture
    "ancient_temple": {"sky": (180, 190, 200), "ground": (160, 140, 110), "accent": (200, 170, 100)},
    "modern_skyscraper": {"sky": (120, 160, 210), "ground": (160, 170, 180), "accent": (80, 140, 200)},
    "european_castle": {"sky": (150, 170, 200), "ground": (140, 130, 110), "accent": (180, 160, 130)},
    "japanese_garden": {"sky": (170, 200, 210), "ground": (60, 100, 50), "accent": (200, 80, 80)},
    "bridge_panorama": {"sky": (140, 180, 220), "ground": (100, 110, 120), "accent": (180, 100, 60)},
    # Weather / sky
    "stormy_clouds": {"sky": (60, 70, 80), "ground": (80, 90, 70), "accent": (200, 200, 210)},
    "rainbow_field": {"sky": (130, 170, 220), "ground": (70, 130, 50), "accent": (255, 100, 100)},
    "foggy_morning": {"sky": (200, 200, 200), "ground": (160, 170, 160), "accent": (180, 180, 180)},
    "starry_night": {"sky": (10, 10, 30), "ground": (20, 30, 20), "accent": (255, 255, 200)},
    "golden_hour": {"sky": (255, 200, 120), "ground": (100, 80, 40), "accent": (255, 180, 80)},
}


def make_landscape_image(scene_name: str, colors: dict, w=640, h=480, variation=0) -> np.ndarray:
    """Generate a synthetic landscape image with the given color palette."""
    img = np.zeros((h, w, 3), dtype=np.uint8)
    rng = np.random.RandomState(hash(scene_name + str(variation)) % (2**31))

    # Sky gradient
    sky = np.array(colors["sky"], dtype=np.float32)
    for y in range(h // 2):
        t = y / (h // 2)
        row_color = sky * (0.7 + 0.3 * (1 - t))
        img[y, :] = np.clip(row_color, 0, 255).astype(np.uint8)

    # Ground
    ground = np.array(colors["ground"], dtype=np.float32)
    for y in range(h // 2, h):
        t = (y - h // 2) / (h // 2)
        row_color = ground * (1.0 - 0.2 * t)
        img[y, :] = np.clip(row_color, 0, 255).astype(np.uint8)

    # Accent shapes (circles/rectangles to simulate objects)
    accent = tuple(int(c) for c in colors["accent"])
    n_shapes = rng.randint(3, 10)
    for _ in range(n_shapes):
        cx, cy = rng.randint(50, w - 50), rng.randint(50, h - 50)
        r = rng.randint(10, 60)
        if rng.random() > 0.5:
            cv2.circle(img, (cx, cy), r, accent, -1)
        else:
            cv2.rectangle(img, (cx - r, cy - r), (cx + r, cy + r), accent, -1)

    # Add noise for texture
    noise = rng.randint(-15, 15, img.shape, dtype=np.int16)
    img = np.clip(img.astype(np.int16) + noise, 0, 255).astype(np.uint8)

    # Burn scene name as text overlay
    cv2.putText(img, scene_name.replace("_", " ").title(), (20, h - 20),
                cv2.FONT_HERSHEY_SIMPLEX, 0.7, (255, 255, 255), 2)

    return img


def make_video(scene_name: str, colors: dict, duration_sec=120, fps=30):
    """Generate a synthetic video (moving shapes) for frame extraction testing."""
    w, h = 640, 480
    out_path = str(MEDIA_DIR / f"{scene_name}.mp4")
    fourcc = cv2.VideoWriter_fourcc(*"mp4v")
    writer = cv2.VideoWriter(out_path, fourcc, fps, (w, h))

    total_frames = duration_sec * fps
    for fi in range(total_frames):
        frame = make_landscape_image(scene_name, colors, w, h, variation=fi // (fps * 10))
        # Animate accent: shift shapes slowly
        t = fi / total_frames
        shift = int(100 * np.sin(2 * np.pi * t))
        M = np.float32([[1, 0, shift], [0, 1, 0]])
        frame = cv2.warpAffine(frame, M, (w, h), borderMode=cv2.BORDER_REFLECT)
        writer.write(frame)

    writer.release()
    return out_path


def main():
    scene_list = list(SCENES.items())

    # Generate ~100 images (multiple variations of scenes)
    print("Generating images...")
    img_count = 0
    for scene_name, colors in scene_list:
        for var in range(3):  # 3 variations each = 105 images
            img = make_landscape_image(scene_name, colors, variation=var)
            fname = f"{scene_name}_v{var}.jpg"
            cv2.imwrite(str(MEDIA_DIR / fname), img, [cv2.IMWRITE_JPEG_QUALITY, 90])
            img_count += 1

    print(f"  Created {img_count} images")

    # Generate ~50 videos (subsets, shorter durations for speed)
    # Use first 17 scenes x 3 durations to get ~51 videos
    print("Generating videos (this takes a moment)...")
    vid_count = 0
    durations = [60, 90, 120]
    for i, (scene_name, colors) in enumerate(scene_list[:17]):
        dur = durations[i % 3]
        make_video(scene_name, colors, duration_sec=dur)
        vid_count += 1
        if vid_count % 5 == 0:
            print(f"  {vid_count} videos...")

    print(f"  Created {vid_count} videos")
    print(f"\nTotal: {img_count} images + {vid_count} videos in {MEDIA_DIR}")


if __name__ == "__main__":
    main()
