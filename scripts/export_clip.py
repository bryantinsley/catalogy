#!/usr/bin/env python3
"""
Export CLIP ViT-H-14 to ONNX format for use with Catalogy.

This script exports both the visual and text encoders from OpenCLIP's
ViT-H-14 model to ONNX files that can be loaded by the catalogy-embed crate.

Prerequisites:
    pip install open_clip_torch torch onnx onnxruntime transformers

Usage:
    python scripts/export_clip.py --output-dir ~/.local/share/catalogy/models

Output files:
    - visual.onnx   : Visual encoder (image -> 1024-dim embedding)
    - text.onnx      : Text encoder (token_ids -> 1024-dim embedding)
    - tokenizer.json : HuggingFace tokenizer for CLIP text encoding
"""

import argparse
import os
from pathlib import Path

import numpy as np
import torch
import open_clip
from open_clip import tokenizer as clip_tokenizer


def export_visual_encoder(model, output_path: Path):
    """Export the visual encoder to ONNX."""
    print("Exporting visual encoder...")

    visual = model.visual
    visual.eval()

    # CLIP ViT-H-14 expects [N, 3, 224, 224] input
    dummy_input = torch.randn(1, 3, 224, 224)

    torch.onnx.export(
        visual,
        dummy_input,
        str(output_path),
        input_names=["pixel_values"],
        output_names=["image_features"],
        dynamic_axes={
            "pixel_values": {0: "batch_size"},
            "image_features": {0: "batch_size"},
        },
        opset_version=17,
        do_constant_folding=True,
    )

    print(f"  Saved to {output_path}")
    print(f"  File size: {output_path.stat().st_size / 1024 / 1024:.1f} MB")

    # Verify
    import onnxruntime as ort

    sess = ort.InferenceSession(str(output_path))
    result = sess.run(None, {"pixel_values": dummy_input.numpy()})
    print(f"  Output shape: {result[0].shape}")
    print(f"  Output dtype: {result[0].dtype}")


def export_text_encoder(model, output_path: Path):
    """Export the text encoder to ONNX."""
    print("Exporting text encoder...")

    # Wrap text encoder for clean export
    class TextEncoder(torch.nn.Module):
        def __init__(self, clip_model):
            super().__init__()
            self.model = clip_model

        def forward(self, input_ids):
            return self.model.encode_text(input_ids)

    text_encoder = TextEncoder(model)
    text_encoder.eval()

    # CLIP text input: [N, 77] int64 token IDs
    dummy_input = torch.zeros(1, 77, dtype=torch.long)
    dummy_input[0, 0] = 49406  # SOT token
    dummy_input[0, 1] = 49407  # EOT token

    torch.onnx.export(
        text_encoder,
        dummy_input,
        str(output_path),
        input_names=["input_ids"],
        output_names=["text_features"],
        dynamic_axes={
            "input_ids": {0: "batch_size"},
            "text_features": {0: "batch_size"},
        },
        opset_version=17,
        do_constant_folding=True,
    )

    print(f"  Saved to {output_path}")
    print(f"  File size: {output_path.stat().st_size / 1024 / 1024:.1f} MB")

    # Verify
    import onnxruntime as ort

    sess = ort.InferenceSession(str(output_path))
    result = sess.run(None, {"input_ids": dummy_input.numpy()})
    print(f"  Output shape: {result[0].shape}")
    print(f"  Output dtype: {result[0].dtype}")


def export_tokenizer(output_path: Path):
    """Export the CLIP tokenizer in HuggingFace format."""
    print("Exporting tokenizer...")

    from transformers import CLIPTokenizerFast

    tokenizer = CLIPTokenizerFast.from_pretrained("openai/clip-vit-large-patch14")
    tokenizer.save_pretrained(str(output_path.parent))

    # The save_pretrained creates multiple files; we want tokenizer.json
    hf_tokenizer_path = output_path.parent / "tokenizer.json"
    if hf_tokenizer_path != output_path:
        hf_tokenizer_path.rename(output_path)

    print(f"  Saved to {output_path}")


def validate_outputs(output_dir: Path, model):
    """Validate that ONNX outputs match PyTorch outputs."""
    print("\nValidating ONNX outputs against PyTorch...")

    import onnxruntime as ort

    # Test visual encoder
    visual_sess = ort.InferenceSession(str(output_dir / "visual.onnx"))
    test_image = torch.randn(1, 3, 224, 224)

    with torch.no_grad():
        pytorch_visual = model.encode_image(test_image).numpy()

    onnx_visual = visual_sess.run(None, {"pixel_values": test_image.numpy()})[0]

    visual_diff = np.abs(pytorch_visual - onnx_visual).max()
    print(f"  Visual encoder max diff: {visual_diff:.6f}")
    assert visual_diff < 1e-3, f"Visual encoder diff too large: {visual_diff}"

    # Test text encoder
    text_sess = ort.InferenceSession(str(output_dir / "text.onnx"))
    test_text = clip_tokenizer.tokenize(["a photo of a cat"])

    with torch.no_grad():
        pytorch_text = model.encode_text(test_text).numpy()

    onnx_text = text_sess.run(None, {"input_ids": test_text.numpy()})[0]

    text_diff = np.abs(pytorch_text - onnx_text).max()
    print(f"  Text encoder max diff: {text_diff:.6f}")
    assert text_diff < 1e-3, f"Text encoder diff too large: {text_diff}"

    print("  Validation passed!")


def main():
    parser = argparse.ArgumentParser(description="Export CLIP ViT-H-14 to ONNX")
    parser.add_argument(
        "--output-dir",
        type=str,
        default=os.path.expanduser("~/.local/share/catalogy/models"),
        help="Directory to save ONNX model files",
    )
    parser.add_argument(
        "--model-name",
        type=str,
        default="ViT-H-14",
        help="OpenCLIP model name (default: ViT-H-14)",
    )
    parser.add_argument(
        "--pretrained",
        type=str,
        default="laion2b_s32b_b79k",
        help="Pretrained weights name",
    )
    parser.add_argument(
        "--skip-validation",
        action="store_true",
        help="Skip output validation",
    )
    args = parser.parse_args()

    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)

    print(f"Loading {args.model_name} ({args.pretrained})...")
    model, _, preprocess = open_clip.create_model_and_transforms(
        args.model_name, pretrained=args.pretrained
    )
    model.eval()

    print(f"Model loaded. Embedding dimension: {model.visual.output_dim}")
    print(f"Output directory: {output_dir}\n")

    export_visual_encoder(model, output_dir / "visual.onnx")
    export_text_encoder(model, output_dir / "text.onnx")
    export_tokenizer(output_dir / "tokenizer.json")

    if not args.skip_validation:
        validate_outputs(output_dir, model)

    print(f"\nDone! Model files saved to {output_dir}")
    print("Set CATALOGY_MODEL_DIR to this path, or place files in ~/.local/share/catalogy/models/")


if __name__ == "__main__":
    main()
