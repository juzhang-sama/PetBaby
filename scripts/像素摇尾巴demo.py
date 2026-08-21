# /// script
# requires-python = ">=3.13"
# dependencies = ["numpy", "Pillow"]
# ///

# How to run:
#   uv run scripts/像素摇尾巴demo.py <body.png>
#
# The runtime demo uses 512x512 RGBA frames. ffmpeg is only used to build the
# review GIF; the individual PNG frames are the source artifact.
from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path
from typing import Final

import numpy as np
from PIL import Image, ImageDraw


LOGICAL_SIZE: Final[int] = 512
FRAME_MS: Final[int] = 120
WAG_OFFSETS: Final[tuple[int, ...]] = (0, 2, 4, 5, 4, 2, 0, -2, -4, -5, -4, -2, 0)
TAIL_TIP_Y: Final[int] = 365
TAIL_ROOT_Y: Final[int] = 432
TAIL_MASK_POINTS: Final[tuple[tuple[int, int], ...]] = (
    (412, 365),
    (471, 365),
    (481, 423),
    (440, 432),
    (407, 412),
)
ANCHOR_BOX: Final[tuple[int, int, int, int]] = (400, 428, 445, 455)
TARGET_BOX: Final[tuple[int, int, int, int]] = (405, 360, 490, 426)


def load_rgba(path: Path) -> np.ndarray:
    image = Image.open(path).convert("RGBA")
    if image.size != (LOGICAL_SIZE, LOGICAL_SIZE):
        image = image.resize((LOGICAL_SIZE, LOGICAL_SIZE), Image.Resampling.BOX)
    return np.asarray(image, dtype=np.int16)


def tail_mask() -> np.ndarray:
    mask_image = Image.new("L", (LOGICAL_SIZE, LOGICAL_SIZE), 0)
    ImageDraw.Draw(mask_image).polygon(TAIL_MASK_POINTS, fill=255)
    return np.asarray(mask_image, dtype=np.uint8) > 0


def tail_wag_frame(source: np.ndarray, mask: np.ndarray, max_offset: int) -> np.ndarray:
    frame = source.copy()
    frame[mask] = 0
    source_y, source_x = np.nonzero(mask)
    for y, x in zip(source_y.tolist(), source_x.tolist(), strict=True):
        progress = min(1.0, max(0.0, (TAIL_ROOT_Y - y) / (TAIL_ROOT_Y - TAIL_TIP_Y)))
        offset = int(round(max_offset * progress))
        destination_x = max(0, min(LOGICAL_SIZE - 1, x + offset))
        frame[y, destination_x] = source[y, x]
    source_alpha = source[:, :, 3] > 0
    frame_alpha = frame[:, :, 3] > 0
    internal_holes = (
        mask
        & ~frame_alpha
        & source_alpha
        & np.roll(source_alpha, 1, axis=1)
        & np.roll(source_alpha, -1, axis=1)
    )
    frame[internal_holes] = source[internal_holes]
    return frame


def changed_pixels(first: np.ndarray, second: np.ndarray, box: tuple[int, int, int, int]) -> int:
    x0, y0, x1, y1 = box
    difference = np.abs(first[y0:y1, x0:x1] - second[y0:y1, x0:x1])
    return int(np.any(difference > 0, axis=2).sum())


def validate_frames(source: np.ndarray, frames: list[np.ndarray]) -> None:
    if len(frames) != len(WAG_OFFSETS):
        raise AssertionError("unexpected tail frame count")
    if not np.array_equal(frames[0], frames[-1]):
        raise AssertionError("tail loop does not return to the neutral frame")
    for frame in frames:
        if frame.shape != source.shape or frame.dtype != source.dtype:
            raise AssertionError("tail frame shape or dtype changed")
        if changed_pixels(source, frame, ANCHOR_BOX) != 0:
            raise AssertionError("tail root anchor changed")
    if max(changed_pixels(source, frame, TARGET_BOX) for frame in frames) <= 20:
        raise AssertionError("tail target region has no visible movement")


def ffmpeg_path() -> str:
    candidates = (
        shutil.which("ffmpeg"),
        r"D:\DevTools\FFmpeg\ffmpeg-8.1.1-full_build\bin\ffmpeg.exe",
    )
    for candidate in candidates:
        if candidate and Path(candidate).exists():
            return candidate
    raise FileNotFoundError("ffmpeg is required to create the review GIF")


def export(source_path: Path) -> Path:
    source = load_rgba(source_path)
    mask = tail_mask()
    frames = [tail_wag_frame(source, mask, offset) for offset in WAG_OFFSETS]
    validate_frames(source, frames)

    output_dir = Path("output/pixel-motion-demo/v7-tail-wag")
    frames_dir = output_dir / "frames"
    review_frames_dir = output_dir / "review-frames"
    frames_dir.mkdir(parents=True, exist_ok=True)
    review_frames_dir.mkdir(parents=True, exist_ok=True)
    review_background = Image.new("RGBA", (LOGICAL_SIZE, LOGICAL_SIZE), (236, 234, 229, 255))
    review_frames: list[Image.Image] = []
    for index, frame in enumerate(frames):
        image = Image.fromarray(frame.astype(np.uint8), "RGBA")
        image.save(frames_dir / f"f{index:02d}.png")
        review_frame = Image.alpha_composite(review_background, image)
        review_frame.save(review_frames_dir / f"f{index:02d}.png")
        review_frames.append(review_frame)

    Image.fromarray(source.astype(np.uint8), "RGBA").resize(
        (1024, 1024), Image.Resampling.NEAREST,
    ).save(output_dir / "rest.png")
    Image.fromarray(frames[3].astype(np.uint8), "RGBA").resize(
        (1024, 1024), Image.Resampling.NEAREST,
    ).save(output_dir / "peak-right.png")
    Image.fromarray(frames[9].astype(np.uint8), "RGBA").resize(
        (1024, 1024), Image.Resampling.NEAREST,
    ).save(output_dir / "peak-left.png")

    contact_sheet = Image.new("RGB", (LOGICAL_SIZE * 3, LOGICAL_SIZE), (236, 234, 229))
    for column, frame_index in enumerate((0, 3, 9)):
        contact_sheet.paste(review_frames[frame_index].convert("RGB"), (column * LOGICAL_SIZE, 0))
    contact_sheet.save(output_dir / "tail-wag-contact-sheet.png")

    gif_path = output_dir / "tail-wag.gif"
    subprocess.run(
        [
            ffmpeg_path(),
            "-y",
            "-framerate",
            f"1000/{FRAME_MS}",
            "-i",
            str(review_frames_dir / "f%02d.png"),
            "-vf",
            "scale=512:512:flags=neighbor,split[s0][s1];"
            "[s0]palettegen=stats_mode=diff[p];"
            "[s1][p]paletteuse=dither=bayer:bayer_scale=4",
            "-loop",
            "0",
            str(gif_path),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    print(f"generated {len(frames)} frames at {frames_dir}")
    print(f"gif: {gif_path}")
    print(f"anchor changes: {max(changed_pixels(source, frame, ANCHOR_BOX) for frame in frames)}")
    print(f"peak target changes: {max(changed_pixels(source, frame, TARGET_BOX) for frame in frames)}")
    return gif_path


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: python scripts/像素摇尾巴demo.py <body.png>")
    export(Path(sys.argv[1]))
