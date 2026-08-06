# -*- coding: utf-8 -*-
"""Single-image part-layer decomposition for generated pet assets.

Stage 1 of the layered-runtime route: a vision model (GPT-4o via the same
aggregation platform as generation) annotates the cut-out pet image with
per-part polygons, pivots and a paint order; pure functions then rasterize
the polygons into masks, resolve overlap by depth order, crop each part to a
tight transparent layer and verify that composing the layers reproduces the
original image.

The output is a set of RGBA part PNGs plus manifest ``parts`` entries that fit
the desktop runtime's ``parts`` contract (role / relativePath / anchor /
pivot / zIndex / deformable / boneId).
"""
import argparse
import base64
import json
import os
import sys
from dataclasses import dataclass
from pathlib import Path

import cv2
import httpx
import numpy as np
from PIL import Image
from scipy.ndimage import distance_transform_edt

try:
    import config
except ModuleNotFoundError:
    from src import config  # type: ignore[no-redef]


PART_ROLES = (
    "body",
    "head",
    "leftEar",
    "rightEar",
    "leftEye",
    "rightEye",
    "tail",
)
DEFAULT_Z_ORDER = (
    "tail",
    "body",
    "head",
    "leftEar",
    "rightEar",
    "leftEye",
    "rightEye",
)
BONE_BY_ROLE = {
    "body": "root",
    "head": "head",
    "leftEar": "leftEar",
    "rightEar": "rightEar",
    "leftEye": "leftEye",
    "rightEye": "rightEye",
    "tail": "tail",
}
PART_COLORS = {
    "body": (255, 80, 80),
    "head": (120, 230, 120),
    "leftEar": (255, 220, 80),
    "rightEar": (220, 160, 255),
    "leftEye": (90, 170, 255),
    "rightEye": (60, 220, 220),
    "tail": (255, 120, 220),
}

LAYER_SYSTEM_PROMPT = (
    "You are a pet asset layering annotator. The image shows a single {species} "
    "on a transparent or plain background, cropped tightly around the pet, "
    "front view. Return JSON with exactly these keys:\n"
    '"parts": an array of 7 objects with roles "body", "head", "leftEar", '
    '"rightEar", "leftEye", "rightEye", "tail".\n'
    'Each part object is {{"role": "...", "polygon": [[x,y], ...], '
    '"pivot": [x, y]}}. Point counts: body/head/tail polygons MUST have 16-24 '
    "points; each ear 10-16 points; each eye 8-12 points. Trace the complete "
    "visible outline of every part densely - do not summarize or simplify, "
    "and do not skip paws, fur tufts, whiskers, or the tail tip.\n"
    '"zOrder": an array of the 7 roles from bottom (painted first) to top.\n'
    "Body includes torso, chest, legs and paws. Head includes face, muzzle and "
    "whiskers but NOT the ears or eyes. Ears are only the ear flaps. Eyes are "
    "only the visible eye areas. Tail is the tail if visible; if the tail is "
    "not visible, use a small region at the lower rear side of the body.\n"
    "All coordinates are normalized 0..1 relative to image width/height, x "
    "from left, y from top; left/right are from the viewer's perspective. "
    "Polygons should generously enclose each part: slightly larger than the "
    "visible part is fine because they are clipped to the pet silhouette. "
    "The union of all polygons must cover every visible pixel of the pet "
    "including whiskers, fur tufts, paws and chest. Keep points inside 0..1.\n"
    "Pivot meanings: body = ground contact point at the bottom center; head = "
    "neck joint at the bottom center of the head; ear = ear base where it "
    "attaches to the head; eye = the exact center of that eye; tail = tail base where it "
    "attaches to the body.\n"
    "Return only the JSON object, no markdown fences."
)


@dataclass(frozen=True)
class PartSpec:
    role: str
    polygon: tuple[tuple[float, float], ...]
    pivot: tuple[float, float]


@dataclass
class ExtractedLayer:
    role: str
    image: Image.Image
    origin: tuple[int, int]
    pivot: tuple[float, float]
    anchor: tuple[float, float]


@dataclass
class LayerSet:
    layers: dict[str, ExtractedLayer]
    parts: list[dict]
    coverage: float
    diff: float
    assigned: dict[str, np.ndarray]


def _clamp01(value: float) -> float:
    return min(1.0, max(0.0, float(value)))


def _parse_polygon(value) -> tuple[tuple[float, float], ...]:
    if not isinstance(value, list) or len(value) < 3:
        raise ValueError("polygon must have at least 3 points")
    points: list[tuple[float, float]] = []
    for item in value:
        if not isinstance(item, (list, tuple)) or len(item) != 2:
            raise ValueError("polygon point must be [x, y]")
        x, y = item
        if not isinstance(x, (int, float)) or not isinstance(y, (int, float)):
            raise ValueError("polygon point must contain numbers")
        points.append((_clamp01(x), _clamp01(y)))
    return tuple(points)


def _parse_pivot(value) -> tuple[float, float]:
    if not isinstance(value, (list, tuple)) or len(value) != 2:
        raise ValueError("pivot must be [x, y]")
    x, y = value
    if not isinstance(x, (int, float)) or not isinstance(y, (int, float)):
        raise ValueError("pivot must contain numbers")
    return (_clamp01(x), _clamp01(y))


def parse_part_layers(
    raw: dict,
    *,
    default_z_order: tuple[str, ...] = DEFAULT_Z_ORDER,
) -> tuple[dict[str, PartSpec], list[str]]:
    """Validate and normalize the vision-model layer annotation.

    Returns (parts_by_role, paint_order bottom-to-top). Coordinates are
    clamped into 0..1; unknown roles are ignored for forward compatibility.
    """
    items = raw.get("parts")
    if not isinstance(items, list) or not items:
        raise ValueError("parts must be a non-empty array")
    parts: dict[str, PartSpec] = {}
    for item in items:
        if not isinstance(item, dict):
            raise ValueError("each part must be an object")
        role = item.get("role")
        if not isinstance(role, str) or role not in PART_ROLES:
            continue
        if role in parts:
            raise ValueError(f"duplicate part role: {role}")
        parts[role] = PartSpec(
            role=role,
            polygon=_parse_polygon(item.get("polygon")),
            pivot=_parse_pivot(item.get("pivot")),
        )
    if not parts:
        raise ValueError("no valid part roles found")
    raw_order = raw.get("zOrder")
    if isinstance(raw_order, list):
        tail_before_body = (
            "tail" in raw_order
            and "body" in raw_order
            and raw_order.index("tail") < raw_order.index("body")
        )
    else:
        tail_before_body = default_z_order.index("tail") < default_z_order.index("body")
    order: list[str] = []
    if tail_before_body and "tail" in parts:
        order.append("tail")
    if "body" in parts:
        order.append("body")
    if not tail_before_body and "tail" in parts:
        order.append("tail")
    for role in ("head", "leftEar", "rightEar", "leftEye", "rightEye"):
        if role in parts:
            order.append(role)
    return parts, order


def rasterize_masks(
    parts: dict[str, PartSpec],
    size: tuple[int, int],
    dilate_px: int = 0,
) -> dict[str, np.ndarray]:
    """Rasterize normalized polygons into boolean masks of ``size``."""
    width, height = size
    masks: dict[str, np.ndarray] = {}
    for role, part in parts.items():
        points = np.array(
            [[x * width, y * height] for x, y in part.polygon],
            dtype=np.int32,
        )
        canvas = np.zeros((height, width), dtype=np.uint8)
        cv2.fillPoly(canvas, [points], 1)
        mask = canvas.astype(bool)
        if dilate_px > 0:
            kernel = cv2.getStructuringElement(
                cv2.MORPH_ELLIPSE, (2 * dilate_px + 1, 2 * dilate_px + 1)
            )
            mask = cv2.dilate(mask.astype(np.uint8), kernel).astype(bool)
        masks[role] = mask
    return masks


def polygon_boxes(
    parts: dict[str, PartSpec],
    size: tuple[int, int],
) -> dict[str, tuple[int, int, int, int]]:
    """Pixel-space bounding boxes of the annotated polygons."""
    width, height = size
    boxes: dict[str, tuple[int, int, int, int]] = {}
    for role, part in parts.items():
        xs = [x * width for x, _y in part.polygon]
        ys = [y * height for _x, y in part.polygon]
        boxes[role] = (
            int(round(min(xs))),
            int(round(min(ys))),
            int(round(max(xs))),
            int(round(max(ys))),
        )
    return boxes


def sam_prompt_boxes(
    parts: dict[str, PartSpec],
    size: tuple[int, int],
) -> dict[str, tuple[int, int, int, int]]:
    """Box prompts for SAM refinement.

    The head box is expanded to the union with both ear boxes so SAM sees the
    full head silhouette (the ears reclaim their own pixels afterwards).
    """
    boxes = polygon_boxes(parts, size)
    if "head" in boxes and ("leftEar" in boxes or "rightEar" in boxes):
        x1, y1, x2, y2 = boxes["head"]
        for ear in ("leftEar", "rightEar"):
            if ear not in boxes:
                continue
            ex1, ey1, ex2, ey2 = boxes[ear]
            x1, y1 = min(x1, ex1), min(y1, ey1)
            x2, y2 = max(x2, ex2), max(y2, ey2)
        boxes["head"] = (x1, y1, x2, y2)
    return boxes


def assign_layer_pixels(
    masks: dict[str, np.ndarray],
    alpha: np.ndarray,
    z_order: list[str],
    *,
    nearest_fallback: bool = True,
    fallback_role: str | None = None,
) -> tuple[dict[str, np.ndarray], float]:
    """Resolve overlapping masks by paint order (top wins).

    With ``nearest_fallback`` every remaining opaque pixel is assigned to the
    closest part mask (distance transform), so a coarse vision polygon never
    leaves holes in the layer set. Alternatively ``fallback_role`` assigns all
    remaining pixels to one role (used with SAM refinement: the body owns
    everything the top parts did not claim, which prevents ear/tail inflation).

    Returns (assigned_masks, coverage) where coverage is the fraction of
    opaque pixels that belong to at least one part.
    """
    remaining = alpha > 0
    assigned: dict[str, np.ndarray] = {}
    # Paint order is bottom -> top, so the topmost layer claims pixels first.
    for role in reversed(z_order):
        mask = masks.get(role)
        if mask is None:
            continue
        part_pixels = mask & remaining
        assigned[role] = part_pixels
        remaining &= ~mask
    if nearest_fallback and remaining.any():
        present = [
            (role, mask)
            for role, mask in masks.items()
            if role in z_order and mask.any()
        ]
        if len(present) > 0:
            if len(present) == 1:
                assigned[present[0][0]] |= remaining
            else:
                distances = np.stack(
                    [distance_transform_edt(~mask) for _, mask in present],
                    axis=0,
                )
                nearest_index = np.argmin(distances, axis=0)
                for index, (role, _mask) in enumerate(present):
                    assigned[role] |= remaining & (nearest_index == index)
            remaining[:] = False
    elif fallback_role in masks and remaining.any():
        assigned[fallback_role] |= remaining
        remaining[:] = False
    total = int(np.count_nonzero(alpha > 0))
    coverage = 1.0 - (int(np.count_nonzero(remaining)) / total) if total else 1.0
    return assigned, coverage


def _mask_center(mask: np.ndarray) -> tuple[int, int]:
    ys, xs = np.where(mask)
    return int(round((xs.min() + xs.max()) / 2)), int(round((ys.min() + ys.max()) / 2))


def _attachment_point(
    mask: np.ndarray,
    anchor_mask: np.ndarray | None,
) -> tuple[int, int]:
    """Point of ``mask`` closest to ``anchor_mask`` (used for joints)."""
    if anchor_mask is not None and anchor_mask.any():
        distances = distance_transform_edt(~anchor_mask)
        ys, xs = np.where(mask)
        if len(ys):
            index = int(np.argmin(distances[ys, xs]))
            return int(xs[index]), int(ys[index])
    x, _y = _mask_center(mask)
    return x, int(np.where(mask)[0].max())


def derive_pivots(
    assigned: dict[str, np.ndarray],
    size: tuple[int, int],
) -> dict[str, tuple[float, float]]:
    """Derive joint pivots from mask geometry instead of trusting the model.

    Eyes pivot at their own center, ears/tail at the attachment point to the
    head/body, head at its bottom center (neck), body at its ground point.
    Returns full-image normalized coordinates (0..1).
    """
    width, height = size
    pivots: dict[str, tuple[float, float]] = {}
    for role, mask in assigned.items():
        if not mask.any():
            continue
        ys, xs = np.where(mask)
        if role in ("leftEye", "rightEye"):
            px, py = _mask_center(mask)
        elif role in ("leftEar", "rightEar"):
            px, py = _attachment_point(mask, assigned.get("head"))
        elif role == "tail":
            px, py = _attachment_point(mask, assigned.get("body"))
        elif role == "head":
            px = int(round((xs.min() + xs.max()) / 2))
            py = int(ys.max())
        else:  # body and anything else
            px = int(round((xs.min() + xs.max()) / 2))
            py = int(ys.max())
        pivots[role] = (px / width, py / height)
    return pivots


def fix_left_right_perspective(
    masks: dict[str, np.ndarray],
) -> dict[str, np.ndarray]:
    """Swap left/right masks when the annotator used the pet's perspective."""
    fixed = dict(masks)
    for left_role, right_role in (("leftEar", "rightEar"), ("leftEye", "rightEye")):
        left = fixed.get(left_role)
        right = fixed.get(right_role)
        if left is None or right is None:
            continue
        if not left.any() or not right.any():
            continue
        left_x, _ = _mask_center(left)
        right_x, _ = _mask_center(right)
        if left_x > right_x:
            fixed[left_role], fixed[right_role] = right, left
    return fixed


def extract_layers(
    image: Image.Image,
    assigned: dict[str, np.ndarray],
    pivots: dict[str, tuple[float, float]],
    pad: int | None = None,
    pad_ratio: float = 0.02,
    min_pad: int = 2,
) -> list[ExtractedLayer]:
    """Crop every assigned mask to a tight RGBA layer with padding."""
    img = image.convert("RGBA")
    arr = np.array(img)
    height, width = arr.shape[:2]
    if pad is None:
        pad = max(min_pad, int(round(pad_ratio * max(width, height))))
    layers: list[ExtractedLayer] = []
    for role, mask in assigned.items():
        ys, xs = np.where(mask)
        if len(ys) == 0:
            continue
        y0, y1 = int(ys.min()), int(ys.max()) + 1
        x0, x1 = int(xs.min()), int(xs.max()) + 1
        top = max(0, y0 - pad)
        left = max(0, x0 - pad)
        bottom = min(height, y1 + pad)
        right = min(width, x1 + pad)
        crop_mask = mask[top:bottom, left:right]
        crop_rgba = arr[top:bottom, left:right]
        crop = np.zeros((bottom - top, right - left, 4), dtype=np.uint8)
        crop[..., :3] = np.where(
            crop_mask[..., None], crop_rgba[..., :3], 0
        )
        crop[..., 3] = np.where(crop_mask, crop_rgba[..., 3], 0)
        pivot_px = pivots.get(role, (0.5, 0.5))
        px = (pivot_px[0] * width, pivot_px[1] * height)
        rel_x = _clamp01((px[0] - left) / max(1, right - left))
        rel_y = _clamp01((px[1] - top) / max(1, bottom - top))
        pivot = (rel_x, rel_y)
        layers.append(
            ExtractedLayer(
                role=role,
                image=Image.fromarray(crop, "RGBA"),
                origin=(left, top),
                pivot=pivot,
                anchor=pivot,
            )
        )
    return layers


def compose_layers(
    layers: dict[str, ExtractedLayer],
    size: tuple[int, int],
) -> Image.Image:
    """Re-compose the full canvas from extracted layers."""
    width, height = size
    canvas = np.zeros((height, width, 4), dtype=np.uint8)
    for layer in layers.values():
        img = np.array(layer.image)
        left, top = layer.origin
        region = canvas[top : top + img.shape[0], left : left + img.shape[1]]
        opaque = img[..., 3:4] > 0
        region[:] = np.where(opaque, img, region)
    return Image.fromarray(canvas, "RGBA")


def layer_diff(original: Image.Image, composed: Image.Image) -> float:
    """Mean absolute RGBA difference over the original's opaque pixels."""
    a = np.array(original.convert("RGBA")).astype(np.int16)
    b = np.array(composed.convert("RGBA")).astype(np.int16)
    opaque = a[..., 3] > 0
    if not opaque.any():
        return 0.0
    return float(np.abs(a - b)[opaque].mean())


def build_layer_set(
    image: Image.Image,
    parts: dict[str, PartSpec],
    z_order: list[str],
    *,
    dilate_px: int = 1,
    nearest_fallback: bool = True,
    sam=None,
    pad: int | None = None,
    pad_ratio: float = 0.02,
    min_pad: int = 2,
) -> LayerSet:
    """Full decomposition: masks -> depth assignment -> layers -> manifest."""
    img = image.convert("RGBA")
    alpha = np.array(img)[..., 3]
    masks = rasterize_masks(parts, img.size, dilate_px=dilate_px)
    masks = fix_left_right_perspective(masks)
    sam_used = sam is not None
    if sam is not None:
        refined = sam.segment_boxes(img, sam_prompt_boxes(parts, img.size))
        for role, mask in refined.items():
            if role in masks and mask.any():
                masks[role] = mask & (alpha > 0)
    assigned, coverage = assign_layer_pixels(
        masks,
        alpha,
        z_order,
        nearest_fallback=nearest_fallback,
        fallback_role="body" if sam_used else None,
    )
    pivots = derive_pivots(assigned, img.size)
    pivots = {
        role: pivots.get(role, part.pivot)
        for role, part in parts.items()
    }
    extracted = extract_layers(
        img, assigned, pivots, pad=pad, pad_ratio=pad_ratio, min_pad=min_pad
    )
    layers = {layer.role: layer for layer in extracted}
    present_order = [role for role in z_order if role in layers]
    ordered = [layers[role] for role in present_order]
    manifest = [
        {
            "role": layer.role,
            "relativePath": f"layers/{layer.role}.png",
            "anchor": {"x": layer.anchor[0], "y": layer.anchor[1]},
            "pivot": {"x": layer.pivot[0], "y": layer.pivot[1]},
            "zIndex": present_order.index(layer.role),
            "deformable": True,
            "boneId": BONE_BY_ROLE[layer.role],
        }
        for layer in ordered
    ]
    composed = compose_layers(layers, img.size)
    return LayerSet(
        layers=layers,
        parts=manifest,
        coverage=coverage,
        diff=layer_diff(img, composed),
        assigned=assigned,
    )


def quality_score(layer_set: LayerSet) -> int:
    """Geometric sanity score for a decomposed layer set (higher is better).

    Rewards: all seven parts present, left/right eye and ear order, ears
    above the head center, head above the body, and tail outside the head.
    """

    def center(role: str) -> tuple[float, float] | None:
        layer = layer_set.layers.get(role)
        if layer is None:
            return None
        arr = np.array(layer.image)
        ys, xs = np.where(arr[..., 3] > 0)
        if len(ys) == 0:
            return None
        return (
            layer.origin[0] + (xs.min() + xs.max()) / 2,
            layer.origin[1] + (ys.min() + ys.max()) / 2,
        )

    def bbox(role: str) -> tuple[float, float, float, float] | None:
        layer = layer_set.layers.get(role)
        if layer is None:
            return None
        arr = np.array(layer.image)
        ys, xs = np.where(arr[..., 3] > 0)
        if len(ys) == 0:
            return None
        return (
            layer.origin[0] + xs.min(),
            layer.origin[1] + ys.min(),
            layer.origin[0] + xs.max(),
            layer.origin[1] + ys.max(),
        )

    score = len(layer_set.layers)
    left_eye, right_eye = center("leftEye"), center("rightEye")
    if left_eye and right_eye and left_eye[0] < right_eye[0]:
        score += 2
    left_ear, right_ear = center("leftEar"), center("rightEar")
    if left_ear and right_ear and left_ear[0] < right_ear[0]:
        score += 2
    head = center("head")
    if head:
        if left_ear and left_ear[1] < head[1]:
            score += 1
        if right_ear and right_ear[1] < head[1]:
            score += 1
        head_box = bbox("head")
        if head_box:
            head_center_y = (head_box[1] + head_box[3]) / 2
            ear_limit = head_center_y + 0.35 * (head_box[3] - head_box[1])
            for ear in ("leftEar", "rightEar"):
                ear_box = bbox(ear)
                if ear_box and ear_box[3] <= ear_limit:
                    score += 1
    body = center("body")
    if head and body and head[1] < body[1]:
        score += 1
    tail = center("tail")
    if head and tail and tail[1] > head[1]:
        score += 1
    return score


def eye_content_ok(
    layer_set: LayerSet,
    image: Image.Image,
    *,
    dark_threshold: int = 110,
    sat_threshold: int = 60,
    min_ratio: float = 0.05,
) -> bool:
    """True when both eye layers actually contain eye-like pixels.

    A mislocated eye layer is usually just fur: it has almost no dark or
    saturated pixels. This check samples the source image at each eye layer's
    position and requires a small fraction of dark/colorful pixels.
    """
    source = np.array(image.convert("RGBA"))
    for role in ("leftEye", "rightEye"):
        layer = layer_set.layers.get(role)
        if layer is None:
            return False
        arr = np.array(layer.image)
        mask = arr[..., 3] > 0
        if not mask.any():
            return False
        left, top = layer.origin
        height, width = mask.shape
        region = source[top : top + height, left : left + width]
        pixels = region[mask][:, :3].astype(np.int16)
        if len(pixels) == 0:
            return False
        luminance = pixels.mean(axis=1)
        saturation = pixels.max(axis=1) - pixels.min(axis=1)
        ratio = float(
            ((luminance < dark_threshold) | (saturation > sat_threshold)).mean()
        )
        if ratio < min_ratio:
            return False
    return True


def _fit_cell(image: Image.Image, cell: int) -> Image.Image:
    image = image.copy()
    image.thumbnail((cell, cell), Image.Resampling.LANCZOS)
    canvas = Image.new("RGBA", (cell, cell), (245, 245, 245, 255))
    offset = ((cell - image.width) // 2, (cell - image.height) // 2)
    canvas.paste(image, offset, image)
    return canvas


def save_layer_set(
    out_dir: Path,
    image: Image.Image,
    layer_set: LayerSet,
    z_order: list[str],
) -> dict:
    """Write layers/*.png, parts.json and a checkerboard preview montage."""
    out_dir = Path(out_dir)
    layers_dir = out_dir / "layers"
    layers_dir.mkdir(parents=True, exist_ok=True)
    paths: dict[str, str] = {}
    for layer in layer_set.layers.values():
        path = layers_dir / f"{layer.role}.png"
        layer.image.save(path)
        paths[layer.role] = str(path)
    (out_dir / "parts.json").write_text(
        json.dumps({"parts": layer_set.parts}, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    original = image.convert("RGBA")
    composed = compose_layers(layer_set.layers, original.size)
    cell = 240
    present = [role for role in z_order if role in layer_set.layers]
    cols = 2 + len(present)
    montage = Image.new(
        "RGBA", (cols * cell, cell), (230, 230, 230, 255)
    )
    montage.paste(_fit_cell(original, cell), (0, 0))
    montage.paste(_fit_cell(composed, cell), (cell, 0))
    for index, role in enumerate(present):
        montage.paste(
            _fit_cell(layer_set.layers[role].image, cell), ((index + 2) * cell, 0)
        )
    preview = out_dir / "preview.png"
    montage.save(preview)
    segmentation = Image.new(
        "RGBA", original.size, (0, 0, 0, 0)
    )
    seg_arr = np.array(segmentation)
    for role, mask in layer_set.assigned.items():
        color = PART_COLORS.get(role, (200, 200, 200))
        seg_arr[mask] = (*color, 255)
    segmentation = Image.fromarray(seg_arr, "RGBA")
    seg_path = out_dir / "segmentation.png"
    segmentation.save(seg_path)
    return {
        "roles": present,
        "coverage": layer_set.coverage,
        "diff": layer_set.diff,
        "layers": paths,
        "partsJson": str(out_dir / "parts.json"),
        "preview": str(preview),
        "segmentation": str(seg_path),
    }


class PartLayerAnalyzer:
    """Vision-model annotation client for part polygons/pivots/zOrder."""

    def __init__(self, key: str, base: str, model: str, timeout: float = 90.0):
        self._key = key
        self._base = base.rstrip("/")
        self._model = model
        self._timeout = timeout

    def _chat_json(
        self,
        system_prompt: str,
        user_text: str,
        photos: list[tuple[bytes, str]],
        max_tokens: int = 4000,
    ) -> dict | None:
        if not self._model or not photos:
            return None
        try:
            response = httpx.post(
                f"{self._base}/v1/chat/completions",
                headers={
                    "Authorization": f"Bearer {self._key}",
                    "Content-Type": "application/json",
                },
                json={
                    "model": self._model,
                    "messages": [
                        {"role": "system", "content": system_prompt},
                        {
                            "role": "user",
                            "content": [
                                {"type": "text", "text": user_text},
                                {
                                    "type": "image_url",
                                    "image_url": {
                                        "url": (
                                            f"data:{photos[0][1]};base64,"
                                            + base64.b64encode(photos[0][0]).decode()
                                        )
                                    },
                                },
                            ],
                        },
                    ],
                    "temperature": 0,
                    "max_tokens": max_tokens,
                    "response_format": {"type": "json_object"},
                },
                timeout=self._timeout,
            )
        except httpx.HTTPError as exc:
            print(f"[layering] request failed: {exc}", flush=True)
            return None
        if response.status_code != 200:
            print(
                f"[layering] returned {response.status_code}: {response.text[:300]}",
                flush=True,
            )
            return None
        try:
            content = response.json()["choices"][0]["message"]["content"]
            data = json.loads(_strip_code_fences(content))
        except (KeyError, IndexError, TypeError, json.JSONDecodeError) as exc:
            print(f"[layering] bad response: {exc}", flush=True)
            return None
        return data if isinstance(data, dict) else None

    def analyze(self, photo: tuple[bytes, str], species: str) -> dict | None:
        return self._chat_json(
            LAYER_SYSTEM_PROMPT.format(species=species),
            f"Species: {species}",
            [photo],
        )


def _strip_code_fences(content: str) -> str:
    text = content.strip()
    if text.startswith("```"):
        lines = text.splitlines()
        if lines and lines[0].startswith("```"):
            lines = lines[1:]
        if lines and lines[-1].strip() == "```":
            lines = lines[:-1]
        text = "\n".join(lines).strip()
    return text


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Decompose a cut-out pet PNG into riggable part layers."
    )
    parser.add_argument("--image", required=True, help="input RGBA cut-out PNG")
    parser.add_argument("--species", default="cat", choices=("cat", "dog"))
    parser.add_argument(
        "--out",
        default="output/layering/result",
        help="output directory (layers/, parts.json, preview.png)",
    )
    parser.add_argument(
        "--model",
        default=os.environ.get("LK888_ANALYZE_MODEL", "gpt-4o"),
        help="vision model for part annotation",
    )
    parser.add_argument("--dilate", type=int, default=1, help="mask dilation px")
    parser.add_argument(
        "--sam",
        dest="sam",
        action="store_true",
        default=None,
        help="use MobileSAM refinement when models are available",
    )
    parser.add_argument(
        "--no-sam",
        dest="sam",
        action="store_false",
        help="disable MobileSAM refinement",
    )
    args = parser.parse_args(argv)

    image_path = Path(args.image)
    image = Image.open(image_path).convert("RGBA")
    raw_bytes = image_path.read_bytes()
    analyzer = PartLayerAnalyzer(
        config.api_key(), config.base_url(), args.model
    )
    sam = None
    if args.sam is not False:
        try:
            from sam_segment import MobileSam
        except ModuleNotFoundError:
            from src.sam_segment import MobileSam  # type: ignore[no-redef]

        sam = MobileSam()
        if not sam.available():
            print(
                "[layering] SAM models not found, using polygon masks only",
                file=sys.stderr,
            )
            sam = None
    best = None
    for attempt in range(1, 3):
        raw = analyzer.analyze((raw_bytes, "image/png"), args.species)
        if raw is None:
            print(
                f"[layering] analyzer returned no annotation (attempt {attempt})",
                file=sys.stderr,
            )
            continue
        try:
            parts, z_order = parse_part_layers(raw)
        except ValueError as exc:
            print(
                f"[layering] annotation rejected (attempt {attempt}): {exc}",
                file=sys.stderr,
            )
            continue
        layer_set = build_layer_set(
            image, parts, z_order, dilate_px=args.dilate, sam=sam
        )
        score = quality_score(layer_set)
        eyes_ok = eye_content_ok(layer_set, image)
        print(
            f"[layering] attempt {attempt} quality score: {score}, "
            f"eyes content ok: {eyes_ok}",
            file=sys.stderr,
        )
        if eyes_ok:
            key = (1, score, attempt)
        else:
            key = (0, score, attempt)
        if best is None or key > best[0]:
            best = (key, eyes_ok, raw, parts, z_order, layer_set)
    if best is None:
        print("[layering] no usable annotation", file=sys.stderr)
        return 1
    key, eyes_ok, raw, parts, z_order, layer_set = best
    score = key[1]
    summary = save_layer_set(Path(args.out), image, layer_set, z_order)
    annotation_path = Path(args.out) / "annotation.json"
    annotation_path.write_text(
        json.dumps(raw, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    summary["annotation"] = str(annotation_path)
    summary["sam"] = sam is not None
    summary["qualityScore"] = score
    summary["eyeCheck"] = eyes_ok
    print(json.dumps(summary, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
