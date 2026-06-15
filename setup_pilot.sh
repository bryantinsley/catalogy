#!/bin/bash

# --- CONFIGURATION ---
USER=$(whoami)
NAS_PATH="/mnt/nas_media/shr/tor/Nah/"  # Your specific NAS path
PILOT_DIR="/home/$USER/catalogy_pilot"
# ---------------------

echo "Setting up Catalogy Pilot environment in $PILOT_DIR..."

# 1. Create Directories
mkdir -p $PILOT_DIR/db
mkdir -p $PILOT_DIR/models
mkdir -p $PILOT_DIR/transcode_staging

# 2. Create the Config File
cat << EOF > $PILOT_DIR/config.toml
[library]
paths = ["$NAS_PATH"]
extensions_image = ["jpg", "jpeg", "png", "webp", "gif", "bmp", "tiff"]
extensions_video = ["mp4", "mov", "avi", "mkv", "webm"]

[database]
catalog_path = "$PILOT_DIR/db/catalog.lance"
state_path = "$PILOT_DIR/db/state.db"

[embedding]
visual_model_path = "$PILOT_DIR/models/visual.onnx"
text_model_path = "$PILOT_DIR/models/textual.onnx"
tokenizer_path = "$PILOT_DIR/models/tokenizer.json"
model_id = "clip-vit-h-14"
model_version = "1"
dimensions = 1024
batch_size = 4
execution_provider = "cuda"

[extraction]
frame_strategy = "adaptive"
scene_threshold = 0.3
max_interval_seconds = 60
frame_max_dimension = 512
dedup_similarity_threshold = 0.95

[ingest]
workers = 2
hash_algorithm = "sha256"

[dedup]
visual_similarity_threshold = 0.92
cross_video_threshold = 0.90

[transcode]
enabled = true
max_resolution = "1080p"
target_codec = "h265"
target_crf = 23
target_container = "mp4"
use_hw_encoder = true
original_policy = "keep"
staging_dir = "$PILOT_DIR/transcode_staging"

[server]
port = 18080
host = "127.0.0.1"
EOF

echo "Configuration file created at $PILOT_DIR/config.toml"
echo "-------------------------------------------------------"
echo "NEXT STEPS:"
echo "1. Ensure your NAS is mounted at /mnt/nas_media"
echo "2. Download the following files and place them in $PILOT_DIR/models/:"
echo "   - visual.onnx (From Marqo/onnx-open_clip-ViT-H-14)"
echo "   - textual.onnx (From Marqo/onnx-open_clip-ViT-H-14)"
echo "   - tokenizer.json (From a standard CLIP repo, e.g., openai/clip-vit-h-14)"
echo "3. Run 'catalogy scan --config $PILOT_DIR/config.toml' to start the pilot."
