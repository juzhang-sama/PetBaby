"""修复 Cubism 纹理集中由抠图噪声形成的小型封闭透明孔。"""

from __future__ import annotations

import sys
import argparse
from pathlib import Path

import cv2
import numpy as np
from PIL import Image


def repair(
    path: Path,
    maximum_area: int = 64,
    samples: tuple[tuple[int, int, int], ...] = (),
) -> tuple[int, int]:
    rgba = np.asarray(Image.open(path).convert("RGBA")).copy()
    transparent = rgba[:, :, 3] < 16
    # 加一圈透明边界后从左上角一次洪泛，可覆盖所有与画布边缘连通的
    # 透明背景；避免在 2048 纹理上逐轮膨胀上千次。
    flood = np.pad(transparent.astype(np.uint8), 1, constant_values=1)
    mask = np.zeros((flood.shape[0] + 2, flood.shape[1] + 2), dtype=np.uint8)
    cv2.floodFill(flood, mask, (0, 0), 2)
    exterior = flood[1:-1, 1:-1] == 2
    enclosed = transparent & ~exterior
    count, labels, stats, _ = cv2.connectedComponentsWithStats(
        enclosed.astype(np.uint8), 4
    )
    repair_mask = np.zeros(transparent.shape, dtype=np.uint8)
    for index in range(1, count):
        if int(stats[index, cv2.CC_STAT_AREA]) <= maximum_area:
            repair_mask[labels == index] = 255
    repaired = int(np.count_nonzero(repair_mask))
    sample_mask = np.zeros(transparent.shape, dtype=np.uint8)
    for x, y, radius in samples:
        if not (0 <= x < rgba.shape[1] and 0 <= y < rgba.shape[0]):
            raise ValueError(f"UV 采样点超出纹理范围: {x},{y}")
        cv2.circle(sample_mask, (x, y), radius, 255, -1)
    sample_mask &= transparent.astype(np.uint8) * 255
    sampled = int(np.count_nonzero(sample_mask))
    repair_mask = np.maximum(repair_mask, sample_mask)
    if repaired or sampled:
        rgba[:, :, :3] = cv2.inpaint(
            rgba[:, :, :3], repair_mask, 3, cv2.INPAINT_TELEA
        )
        rgba[:, :, 3][repair_mask > 0] = 255
        Image.fromarray(rgba, "RGBA").save(path)
    return repaired, sampled


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("path", type=Path)
    parser.add_argument(
        "--sample",
        action="append",
        default=[],
        metavar="X,Y,RADIUS",
        help="只修补指定 UV 采样点半径内的透明像素，可重复使用",
    )
    args = parser.parse_args()
    path = args.path
    if not path.is_file():
        print(f"纹理文件不存在: {path}", file=sys.stderr)
        return 2
    try:
        samples = tuple(tuple(map(int, value.split(","))) for value in args.sample)
        if any(len(sample) != 3 or sample[2] < 0 for sample in samples):
            raise ValueError
    except ValueError:
        print("--sample 必须是 X,Y,RADIUS 三个非负整数", file=sys.stderr)
        return 2
    enclosed, sampled = repair(path, samples=samples)
    print(f"repaired_enclosed_transparent_pixels={enclosed}")
    print(f"repaired_diagnosed_sample_pixels={sampled}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
