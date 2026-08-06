# -*- coding: utf-8 -*-
"""MobileSAM (ONNX) mask refinement for part-layer decomposition.

The vision model supplies coarse per-part polygons; this module turns each
polygon's bounding box into a SAM box prompt and produces a precise mask.
The encoder runs once per image; the decoder runs once per part.

Models (Apache-2.0, ~45 MB total) are downloaded from Hugging Face:
https://huggingface.co/Heliosoph/sam-onnx
into ``models/sam-onnx/`` (gitignored). Without the files the module simply
reports ``available() == False`` and the pipeline falls back to polygons.
"""
from pathlib import Path

import numpy as np
from PIL import Image

MODEL_FILES = (
    "mobile_sam_image_encoder.onnx",
    "sam_mask_decoder_single.onnx",
)
MEAN = np.array([123.675, 116.28, 103.53], dtype=np.float32)
STD = np.array([58.395, 57.12, 57.375], dtype=np.float32)
MODEL_SIZE = 1024


def default_model_dir() -> Path:
    return Path(__file__).resolve().parent.parent / "models" / "sam-onnx"


def preprocess_for_encoder(image: Image.Image) -> tuple[np.ndarray, float]:
    """Resize+pad to 1024x1024 (right/bottom) and apply SAM normalization.

    Returns (HWC float32 array ready for the ONNX encoder, scale factor).
    """
    rgb = image.convert("RGB")
    width, height = rgb.size
    scale = MODEL_SIZE / max(width, height)
    new_size = (round(width * scale), round(height * scale))
    resized = rgb.resize(new_size, Image.Resampling.BILINEAR)
    canvas = Image.new("RGB", (MODEL_SIZE, MODEL_SIZE), (0, 0, 0))
    canvas.paste(resized, (0, 0))
    arr = np.array(canvas).astype(np.float32)
    return (arr - MEAN) / STD, scale


def scale_boxes_to_model(
    boxes: dict[str, tuple[float, float, float, float]],
    scale: float,
) -> dict[str, tuple[float, float, float, float]]:
    return {
        role: tuple(value * scale for value in box)
        for role, box in boxes.items()
    }


class MobileSam:
    """Thin ONNX Runtime wrapper around the MobileSAM encoder + decoder."""

    def __init__(self, model_dir: Path | None = None, providers=None):
        self._model_dir = Path(model_dir) if model_dir else default_model_dir()
        self._providers = providers or ["CPUExecutionProvider"]
        self._encoder = None
        self._decoder = None

    def available(self) -> bool:
        return all((self._model_dir / name).exists() for name in MODEL_FILES)

    def _load(self):
        if self._encoder is None and self.available():
            import onnxruntime as ort

            self._encoder = ort.InferenceSession(
                str(self._model_dir / MODEL_FILES[0]),
                providers=self._providers,
            )
            self._decoder = ort.InferenceSession(
                str(self._model_dir / MODEL_FILES[1]),
                providers=self._providers,
            )
        return self._encoder, self._decoder

    def segment_boxes(
        self,
        image: Image.Image,
        boxes: dict[str, tuple[float, float, float, float]],
    ) -> dict[str, np.ndarray]:
        """Return a boolean mask per role, in original image coordinates.

        Masks are intersected with the pet alpha channel. Roles whose box is
        degenerate are skipped; empty results are returned as empty masks.
        """
        encoder, decoder = self._load()
        if encoder is None or decoder is None:
            return {}
        if not boxes:
            return {}
        try:
            arr, scale = preprocess_for_encoder(image)
            embeddings = encoder.run(None, {"input_image": arr})[0]
        except Exception as exc:  # noqa: BLE001 - degrade gracefully
            print(f"[sam] encode failed, falling back to polygons: {exc}", flush=True)
            return {}
        width, height = image.size
        alpha = np.array(image.convert("RGBA"))[..., 3] > 0
        scaled = scale_boxes_to_model(boxes, scale)
        results: dict[str, np.ndarray] = {}
        for role, box in scaled.items():
            x1, y1, x2, y2 = box
            if x2 <= x1 or y2 <= y1:
                results[role] = np.zeros((height, width), dtype=bool)
                continue
            try:
                outputs = decoder.run(
                    None,
                    {
                        "image_embeddings": embeddings,
                        "point_coords": np.array(
                            [[[x1, y1], [x2, y2]]], dtype=np.float32
                        ),
                        "point_labels": np.array([[2, 3]], dtype=np.float32),
                        "mask_input": np.zeros((1, 1, 256, 256), dtype=np.float32),
                        "has_mask_input": np.array([0], dtype=np.float32),
                        "orig_im_size": np.array([height, width], dtype=np.float32),
                    },
                )
            except Exception as exc:  # noqa: BLE001
                print(
                    f"[sam] decode failed for {role}, keeping polygon: {exc}",
                    flush=True,
                )
                results[role] = np.zeros((height, width), dtype=bool)
                continue
            low_res = outputs[0]
            mask = low_res[0, 0] > 0
            results[role] = mask & alpha
        return results
