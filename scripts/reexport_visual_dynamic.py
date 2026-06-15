#!/usr/bin/env python3
"""
Re-export the CLIP ViT-H-14 *visual* encoder to ONNX with a genuinely dynamic
batch axis.

The previously-deployed visual.onnx accepts a dynamic batch on its input but an
internal `reshape(seq, dim)` had its batch dimension baked to 1 by the classic
TorchScript tracer's constant folding, so any batch>1 fails at the
`gemm_input_reshape` node. The TorchDynamo-based exporter (torch>=2.x) traces
shapes symbolically and preserves the dynamic batch through internal reshapes.

Offline: weights are cached under ~/.cache/huggingface; HF_HUB_OFFLINE is set.

Usage:
    python scripts/reexport_visual_dynamic.py --out-dir /tmp/catalogy-models-new
"""
import argparse
import os

os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")

from pathlib import Path

import numpy as np
import torch
import open_clip


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out-dir", required=True)
    ap.add_argument("--model-name", default="ViT-H-14")
    ap.add_argument("--pretrained", default="laion2b_s32b_b79k")
    args = ap.parse_args()

    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    out_path = out_dir / "visual.onnx"

    print(f"Loading {args.model_name} ({args.pretrained})...")
    model, _, _ = open_clip.create_model_and_transforms(
        args.model_name, pretrained=args.pretrained
    )
    model.eval()
    visual = model.visual
    visual.eval()

    # A wrapper makes the traced callable a clean image_features -> features fn.
    class VisualEncoder(torch.nn.Module):
        def __init__(self, v):
            super().__init__()
            self.v = v

        def forward(self, pixel_values):
            return self.v(pixel_values)

    enc = VisualEncoder(visual).eval()

    dummy = torch.randn(2, 3, 224, 224)  # batch=2 so batch is never a trivial 1
    batch = torch.export.Dim("batch", min=1, max=4096)

    print("Exporting visual encoder with dynamo (symbolic dynamic batch)...")
    onnx_prog = torch.onnx.export(
        enc,
        (dummy,),
        dynamo=True,
        input_names=["pixel_values"],
        output_names=["image_features"],
        dynamic_shapes={"pixel_values": {0: batch}},
    )
    onnx_prog.optimize()
    onnx_prog.save(str(out_path))
    print(f"  Saved {out_path} ({out_path.stat().st_size/1e6:.1f} MB header)")

    # --- Validate: shapes are dynamic, batch>1 runs, and rows match PyTorch ---
    import onnx
    import onnxruntime as ort

    m = onnx.load(str(out_path), load_external_data=False)
    in_dims = [
        (d.dim_param or d.dim_value)
        for d in m.graph.input[0].type.tensor_type.shape.dim
    ]
    out_dims = [
        (d.dim_param or d.dim_value)
        for d in m.graph.output[0].type.tensor_type.shape.dim
    ]
    print(f"  input  dims: {in_dims}")
    print(f"  output dims: {out_dims}")

    sess = ort.InferenceSession(str(out_path), providers=["CPUExecutionProvider"])
    for bs in (1, 2, 8):
        x = torch.randn(bs, 3, 224, 224)
        onnx_out = sess.run(None, {"pixel_values": x.numpy()})[0]
        with torch.no_grad():
            torch_out = visual(x).numpy()
        diff = np.abs(onnx_out - torch_out).max()
        print(f"  batch={bs}: onnx {onnx_out.shape}  max|onnx-torch|={diff:.6f}")
        assert onnx_out.shape == (bs, torch_out.shape[1]), "wrong output shape"
        assert diff < 2e-3, f"numerical drift too large at batch {bs}: {diff}"

    print("\nValidation passed — dynamic batch works and matches PyTorch.")


if __name__ == "__main__":
    main()
