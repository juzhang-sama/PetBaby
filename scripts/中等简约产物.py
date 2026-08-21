from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from collections.abc import Sequence

import numpy as np
from PIL import Image, ImageDraw, ImageFont

LOGICAL_GRID_SIZE = 160
OUTPUT_SIZE = 1024
GRID_SCALE = 6


@dataclass(frozen=True, slots=True)
class ActionExportSpec:
    action_id: str
    output_root: Path
    duration_ms: int


@dataclass(frozen=True, slots=True)
class ActionArtifact:
    gif_path: Path
    frame_paths: tuple[Path, ...]
    peak_path: Path
    peak_frame_index: int


def load_logical_rgba(path: Path) -> np.ndarray:
    with Image.open(path) as image:
        logical = image.convert("RGBA").resize(
            (LOGICAL_GRID_SIZE, LOGICAL_GRID_SIZE),
            Image.Resampling.NEAREST,
        )
    return np.asarray(logical, dtype=np.uint8)


def export_action(
    spec: ActionExportSpec, frames: Sequence[np.ndarray]
) -> ActionArtifact:
    if not frames:
        raise ValueError("action export requires frames")  # noqa: GENERIC_ERR_OK
    action_root = spec.output_root / spec.action_id
    frame_root = action_root / "frames"
    frame_root.mkdir(parents=True, exist_ok=True)
    frame_paths = tuple(frame_root / f"f{index:02d}.png" for index in range(len(frames)))
    logical_images = tuple(Image.fromarray(frame, "RGBA") for frame in frames)
    for image, path in zip(logical_images, frame_paths, strict=True):
        image.resize((OUTPUT_SIZE, OUTPUT_SIZE), Image.Resampling.NEAREST).save(path)
    changed = tuple(
        int(np.any(frame != frames[0], axis=2).sum()) for frame in frames
    )
    peak_index = int(np.argmax(changed))
    peak_path = action_root / "峰值帧.png"
    logical_images[peak_index].resize(
        (OUTPUT_SIZE, OUTPUT_SIZE), Image.Resampling.NEAREST
    ).save(peak_path)
    background = Image.new("RGBA", (LOGICAL_GRID_SIZE, LOGICAL_GRID_SIZE), (236, 234, 229, 255))
    review_frames = tuple(
        Image.alpha_composite(background, image).convert("P", palette=Image.Palette.ADAPTIVE)
        for image in logical_images
    )
    gif_path = action_root / f"{spec.action_id}.gif"
    review_frames[0].save(
        gif_path,
        save_all=True,
        append_images=review_frames[1:],
        duration=spec.duration_ms,
        loop=0,
        disposal=2,
    )
    return ActionArtifact(gif_path, frame_paths, peak_path, peak_index)


def write_grid_preview(source: np.ndarray, path: Path) -> Path:
    logical = Image.fromarray(source, "RGBA")
    background = Image.new("RGBA", logical.size, (248, 248, 246, 255))
    canvas = Image.alpha_composite(background, logical).resize(
        (LOGICAL_GRID_SIZE * GRID_SCALE, LOGICAL_GRID_SIZE * GRID_SCALE),
        Image.Resampling.NEAREST,
    )
    draw = ImageDraw.Draw(canvas)
    for coordinate in range(0, LOGICAL_GRID_SIZE + 1, 10):
        position = min(canvas.width - 1, coordinate * GRID_SCALE)
        draw.line((position, 0, position, canvas.height - 1), fill=(60, 70, 76, 100))
        draw.line((0, position, canvas.width - 1, position), fill=(60, 70, 76, 100))
        if coordinate < LOGICAL_GRID_SIZE:
            draw.text((position + 2, 2), str(coordinate), fill=(20, 25, 28, 255))
            draw.text((2, position + 2), str(coordinate), fill=(20, 25, 28, 255))
    path.parent.mkdir(parents=True, exist_ok=True)
    canvas.convert("RGB").save(path)
    return path


def write_overview(
    entries: Sequence[tuple[str, Path]],
    path: Path,
    *,
    columns: int | None = None,
) -> Path:
    cell_size = 512
    label_height = 32
    column_count = columns or len(entries)
    row_count = (len(entries) + column_count - 1) // column_count
    images = []
    for _, source_path in entries:
        with Image.open(source_path) as source:
            image = source.convert("RGBA")
            image.thumbnail((cell_size, cell_size), Image.Resampling.LANCZOS)
        images.append(image)
    tile_height = max(image.height for image in images)
    canvas = Image.new(
        "RGB",
        (cell_size * column_count, (tile_height + label_height) * row_count),
        (244, 244, 242),
    )
    draw = ImageDraw.Draw(canvas)
    font_path = Path("C:/Windows/Fonts/msyh.ttc")
    font = (
        ImageFont.truetype(str(font_path), 18)
        if font_path.is_file()
        else ImageFont.load_default()
    )
    for index, ((label, _), image) in enumerate(zip(entries, images, strict=True)):
        row, column = divmod(index, column_count)
        tile_x = column * cell_size
        tile_y = row * (tile_height + label_height)
        tile = Image.new("RGBA", (cell_size, tile_height), (248, 248, 246, 255))
        offset = ((cell_size - image.width) // 2, (tile_height - image.height) // 2)
        tile.alpha_composite(image, offset)
        canvas.paste(tile.convert("RGB"), (tile_x, tile_y + label_height))
        draw.text((tile_x + 8, tile_y + 6), label, fill=(28, 32, 35), font=font)
    path.parent.mkdir(parents=True, exist_ok=True)
    canvas.save(path)
    return path


__all__ = [
    "ActionArtifact",
    "ActionExportSpec",
    "export_action",
    "load_logical_rgba",
    "write_grid_preview",
    "write_overview",
]
