# -*- coding: utf-8 -*-
"""Background removal post-processing for generated pet images."""
from PIL import Image, ImageFilter
import numpy as np


class BackgroundRemovalError(Exception):
    pass


def estimate_background_color(image: Image.Image) -> tuple[int, int, int]:
    """Estimate the dominant background color from the image borders."""
    rgb = image.convert("RGB")
    w, h = rgb.size
    sample = np.array(rgb)
    border = np.concatenate(
        [
            sample[: max(1, h // 20)].reshape(-1, 3),
            sample[-max(1, h // 20):].reshape(-1, 3),
            sample[:, : max(1, w // 20)].reshape(-1, 3),
            sample[:, -max(1, w // 20):].reshape(-1, 3),
        ]
    )
    median = np.median(border, axis=0).astype(int)
    return tuple(median)


def remove_background_chroma(image: Image.Image, tolerance: int = 30) -> Image.Image:
    """Chroma-key removal: assumes a uniform background color."""
    bg = estimate_background_color(image)
    rgb = image.convert("RGB")
    arr = np.array(rgb).astype(np.float32)
    dist = np.abs(arr - np.array(bg)).sum(axis=2)
    alpha = np.clip((dist - tolerance) / 60.0, 0.0, 1.0)
    rgba = np.dstack(
        [
            np.array(rgb),
            (alpha * 255).astype(np.uint8),
        ]
    )
    out = Image.fromarray(rgba, "RGBA")
    # soften the alpha edge
    alpha_layer = out.split()[3].filter(ImageFilter.GaussianBlur(radius=1.5))
    out.putalpha(alpha_layer)
    return out


def is_uniform_background(image: Image.Image, tolerance: int = 25) -> bool:
    """True when the border colors are within tolerance of each other."""
    bg = estimate_background_color(image)
    rgb = image.convert("RGB")
    w, h = rgb.size
    sample = np.array(rgb)
    border = np.concatenate(
        [
            sample[: max(1, h // 20)].reshape(-1, 3),
            sample[-max(1, h // 20):].reshape(-1, 3),
            sample[:, : max(1, w // 20)].reshape(-1, 3),
            sample[:, -max(1, w // 20):].reshape(-1, 3),
        ]
    )
    return np.all(np.abs(border - np.array(bg)) <= tolerance)


def remove_background_rembg(image: Image.Image) -> Image.Image:
    """Background removal via rembg (u2net). Downloads the model on first use."""
    try:
        from rembg import remove
    except ImportError as exc:
        raise BackgroundRemovalError("rembg not installed") from exc
    result = remove(image.convert("RGBA"))
    if result is None:
        raise BackgroundRemovalError("rembg returned no result")
    return result


def remove_background(image: Image.Image, method: str = "auto") -> Image.Image:
    """Remove the background. auto: chroma-key when uniform, else rembg."""
    if method == "chroma":
        return remove_background_chroma(image)
    if method == "rembg":
        return remove_background_rembg(image)
    if method != "auto":
        raise BackgroundRemovalError(f"unknown method: {method}")
    if is_uniform_background(image):
        return remove_background_chroma(image)
    return remove_background_rembg(image)
