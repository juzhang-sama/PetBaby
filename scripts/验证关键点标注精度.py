# -*- coding: utf-8 -*-
"""验证 gpt-4o 从宠物像素图标关键点的坐标精度。

目标：量化「AI 理解（标关键点）+ 程序执行（合成帧）」这条路能否走通。
方法：拿三只已有手工标注的猫母版图，让 gpt-4o 标 8 个关键点（归一化 0..1），
      换算回 160 逻辑网格，与手工标注对比算误差。
判定：≤2px 达标 / 2~5px 需局部精修 / >5px 不可用。
"""
from __future__ import annotations

import json
import sys
from collections import deque
from pathlib import Path

import httpx
import numpy as np
from PIL import Image, ImageDraw

REPO = Path(__file__).resolve().parents[1]
SRC = REPO / "services" / "appearance-generation" / "src"
sys.path.insert(0, str(SRC))
sys.path.insert(0, str(REPO / "scripts"))

from photo_avatar_backend.config import BackendConfig  # noqa: E402
from photo_avatar_backend.lk888_client import Lk888Client  # noqa: E402

GRID = 160
BASE = REPO / "output" / "中等简约像素标准验收-2026-08-21"
PETS = ["01-longhair-black-white", "02-round-tabby", "03-sleek-black"]


def _point(x: float, y: float) -> dict:
    return {"x": round(float(x), 4), "y": round(float(y), 4)}


KEYPOINT_SCHEMA = {
    "type": "object",
    "additionalProperties": False,
    "properties": {
        "headTop": {
            "type": "object", "additionalProperties": False,
            "properties": {"x": {"type": "number"}, "y": {"type": "number"}},
            "required": ["x", "y"],
        },
        "leftEye": {
            "type": "object", "additionalProperties": False,
            "properties": {"x": {"type": "number"}, "y": {"type": "number"}},
            "required": ["x", "y"],
        },
        "rightEye": {
            "type": "object", "additionalProperties": False,
            "properties": {"x": {"type": "number"}, "y": {"type": "number"}},
            "required": ["x", "y"],
        },
        "tailRoot": {
            "type": "object", "additionalProperties": False,
            "properties": {"x": {"type": "number"}, "y": {"type": "number"}},
            "required": ["x", "y"],
        },
        "tailTip": {
            "type": "object", "additionalProperties": False,
            "properties": {"x": {"type": "number"}, "y": {"type": "number"}},
            "required": ["x", "y"],
        },
        "bodyLeftX": {"type": "number"},
        "bodyRightX": {"type": "number"},
        "groundCenter": {
            "type": "object", "additionalProperties": False,
            "properties": {"x": {"type": "number"}, "y": {"type": "number"}},
            "required": ["x", "y"],
        },
    },
    "required": [
        "headTop", "leftEye", "rightEye", "tailRoot",
        "tailTip", "bodyLeftX", "bodyRightX", "groundCenter",
    ],
}

PROMPT = (
    "这是一张像素风格的宠物猫图（正视坐姿，透明背景，猫是主体）。"
    "请仔细看图，标出 8 个关键点，坐标用归一化值（0~1，相对图像左上角，x 向右、y 向下）："
    "headTop=头顶最高点的中心；"
    "leftEye=画面左侧那只眼睛的中心；rightEye=画面右侧那只眼睛的中心；"
    "tailRoot=尾巴连接身体的位置（尾巴根部）；"
    "tailTip=尾巴最远端（离身体最远的那个点）；"
    "bodyLeftX=身体轮廓最左侧的 x 坐标；bodyRightX=身体轮廓最右侧的 x 坐标（含尾巴）；"
    "groundCenter=猫的脚接触地面的位置中心。"
)


def load_env() -> dict:
    env = {}
    env_file = REPO / "services" / "appearance-generation" / ".env"
    for line in env_file.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            k, v = line.split("=", 1)
            env[k.strip()] = v.strip()
    return env


def ground_truth(pid: str) -> dict:
    """从手工标注 + alphaBounds 推 ground truth（160 网格坐标）。"""
    from 中等简约产物 import load_logical_rgba  # noqa: F401
    from 中等简约动作 import MotionAnnotation  # noqa: F401

    a = load_logical_rgba(BASE / pid / "母版.png")
    alpha = a[:, :, 3] > 0
    ys, xs = np.where(alpha)
    L, T, R, B = xs.min(), ys.min(), xs.max(), ys.max()

    ann = MotionAnnotation.model_validate_json(
        (BASE / "annotations" / f"{pid}.json").read_text(encoding="utf-8")
    )
    le = ann.eyes["left"]
    re = ann.eyes["right"]
    ga = ann.ground_anchors[0]

    # 尾巴尖：在 tail.mask 内，从 root 出发 BFS 测地距离最远的像素
    poly = Image.new("L", (GRID, GRID), 0)
    ImageDraw.Draw(poly).polygon(ann.tail.mask, fill=255)
    owned = (np.asarray(poly) > 0) & alpha
    root = ann.tail.root
    dist = np.full((GRID, GRID), -1, np.int32)
    ry, rx = root[1], root[0]
    if not owned[ry, rx]:
        oy, ox = np.where(owned)
        d2 = (ox - root[0]) ** 2 + (oy - root[1]) ** 2
        k = int(np.argmin(d2))
        ry, rx = int(oy[k]), int(ox[k])
    dist[ry, rx] = 0
    q = deque([(ry, rx)])
    while q:
        y, x = q.popleft()
        for ny, nx in ((y - 1, x), (y, x - 1), (y, x + 1), (y + 1, x)):
            if 0 <= ny < GRID and 0 <= nx < GRID and owned[ny, nx] and dist[ny, nx] < 0:
                dist[ny, nx] = dist[y, x] + 1
                q.append((ny, nx))
    tip_y, tip_x = np.unravel_index(int(np.argmax(dist)), dist.shape)

    return {
        "headTop": ((L + R) // 2, T),
        "leftEye": ((le[0] + le[2]) // 2, (le[1] + le[3]) // 2),
        "rightEye": ((re[0] + re[2]) // 2, (re[1] + re[3]) // 2),
        "tailRoot": root,
        "tailTip": (int(tip_x), int(tip_y)),
        "bodyLeftX": int(L),
        "bodyRightX": int(R),
        "groundCenter": ((ga[0] + ga[2]) // 2, (ga[1] + ga[3]) // 2),
    }


def main() -> int:
    env = load_env()
    env["PHOTO_AVATAR_BACKEND_STATE_DIR"] = str(REPO / "output" / "photo-avatar-backend")
    config = BackendConfig.from_env(env)
    client = Lk888Client(config, httpx.Client(timeout=90))

    rows = []
    for pid in PETS:
        png = (BASE / pid / "母版.png").read_bytes()
        result = client.analyze_json(PROMPT, [png], KEYPOINT_SCHEMA)
        gt = ground_truth(pid)

        print(f"\n=== {pid} ===")
        errors = {}
        for key in ["headTop", "leftEye", "rightEye", "tailRoot", "tailTip", "groundCenter"]:
            mx = round(float(result[key]["x"]) * GRID)
            my = round(float(result[key]["y"]) * GRID)
            gx, gy = gt[key]
            err = float(np.hypot(mx - gx, my - gy))
            errors[key] = err
            print(f"  {key:12s} 模型=({mx:3d},{my:3d}) 手工=({gx:3d},{gy:3d}) 误差={err:.1f}px")
        for key in ["bodyLeftX", "bodyRightX"]:
            mv = round(float(result[key]) * GRID)
            gv = gt[key]
            err = abs(mv - gv)
            errors[key] = err
            print(f"  {key:12s} 模型={mv:3d} 手工={gv:3d} 误差={err}px")

        errs = list(errors.values())
        print(f"  → 平均={np.mean(errs):.1f}px 最大={np.max(errs):.1f}px")

        rows.append({
            "pet": pid,
            "mean_px": round(float(np.mean(errs)), 1),
            "max_px": round(float(np.max(errs)), 1),
            "errors": {k: round(v, 1) for k, v in errors.items()},
        })

    out = REPO / "output" / "关键点标注精度验证.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(rows, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"\n结果已写入 {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
