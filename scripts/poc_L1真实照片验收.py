"""L1 真实照片验收：真实照片 → pixel 全程 → 可直接肉眼判定的分身图。

**为什么存在**：现役 pixel / schema 3 路线从未做过真实照片验收。桌面 UI 必须人工启动、
Rust 层已由测试覆盖，所以验收只需跑后端两步（analyzeIdentity / generatePixelAvatar）。
本脚本直接以进程内方式调这两步，复用**真实**契约解析与**真实** lk888 客户端，
不经过 HTTP 后端、不经过桌面 UI。验收判据见
`docs/验证记录/L1真实照片验收协议-2026-09-11.md`。

用法（env 里必须先有真实 LK888_API_KEY）：

    D:/DevTools/Python312/python.exe scripts/poc_L1真实照片验收.py \
        --label 短毛猫 --photo "C:/Users/Administrator/Desktop/果冻.jpg"

同一只宠物多张照片就重复 `--photo`。每只宠物跑一次脚本。
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import sys
import time
import uuid
from io import BytesIO
from pathlib import Path

import httpx
from PIL import Image

REPO_ROOT = Path(__file__).resolve().parent.parent
SRC_ROOT = REPO_ROOT / "services" / "appearance-generation" / "src"
sys.path.insert(0, str(SRC_ROOT))

from photo_avatar_backend.config import BackendConfig  # noqa: E402
from photo_avatar_backend.contracts import PixelStepRequest  # noqa: E402
from photo_avatar_backend.lk888_client import Lk888Client  # noqa: E402
from photo_avatar_backend.pixel_avatar import (  # noqa: E402
    analyze_pixel_identity,
    generate_pixel_avatar,
)
from photo_avatar_backend.pixel_style import (  # noqa: E402
    PIXEL_STYLE_V1_ID,
    load_pixel_style_pack,
)

CONSENT_VERSION = "photo-avatar-third-party-ai-lk888-no-delete-v2"


def load_config() -> BackendConfig:
    """真实配置。PHOTO_AVATAR_BACKEND_TOKEN 只用于满足校验（本脚本不发 HTTP 到后端）。"""
    env = dict(os.environ)
    if not env.get("PHOTO_AVATAR_BACKEND_TOKEN", "").strip():
        env["PHOTO_AVATAR_BACKEND_TOKEN"] = f"local-{uuid.uuid4().hex}"
        print("[配置] PHOTO_AVATAR_BACKEND_TOKEN 未设置，本进程内用随机值占位（不用于任何网络请求）")
    return BackendConfig.from_env(env)


def source_image_payload(photo: Path, index: int) -> dict[str, object]:
    """转 PNG 并算出契约要求的 sha256 / 宽高。输入侧只接受 PNG。"""
    with Image.open(photo) as opened:
        rgb = opened.convert("RGB")
        buffer = BytesIO()
        rgb.save(buffer, format="PNG")
        width, height = rgb.size
    raw = buffer.getvalue()
    return {
        "sourceId": f"photo-{index}",
        "pngBase64": base64.b64encode(raw).decode("ascii"),
        "sha256": hashlib.sha256(raw).hexdigest(),
        "width": width,
        "height": height,
    }


def step_payload(
    *,
    step: str,
    session_id: str,
    provider_session_id: str | None,
    sources: list[dict[str, object]],
    profile: dict[str, object] | None,
) -> dict[str, object]:
    return {
        "route": "pixel-v1",
        "styleProfileId": PIXEL_STYLE_V1_ID,
        "sessionId": session_id,
        "revision": 0,
        "providerSessionId": provider_session_id,
        "step": step,
        "attempt": 1,
        "consentVersion": CONSENT_VERSION,
        "sourceImages": sources,
        "profile": profile,
        "modification": None,
        "lockedTraits": [],
    }


def run_one(
    *,
    label: str,
    photos: list[Path],
    client: Lk888Client,
    style: object,
    out_dir: Path,
) -> dict[str, object]:
    session_id = f"l1-{uuid.uuid4().hex[:12]}"
    provider_session_id = f"local-provider-{uuid.uuid4().hex[:12]}"
    sources = [source_image_payload(photo, index) for index, photo in enumerate(photos, start=1)]
    print(f"[{label}] 照片 {len(photos)} 张 → PNG 尺寸 " +
          ", ".join(f"{item['width']}x{item['height']}" for item in sources))

    analyze_request = PixelStepRequest.parse(
        step_payload(
            step="analyzeIdentity",
            session_id=session_id,
            provider_session_id=provider_session_id,
            sources=sources,
            profile=None,
        )
    )
    started = time.monotonic()
    profile = analyze_pixel_identity(analyze_request, client=client)
    analyze_seconds = time.monotonic() - started

    generate_request = PixelStepRequest.parse(
        step_payload(
            step="generatePixelAvatar",
            session_id=session_id,
            provider_session_id=provider_session_id,
            sources=sources,
            profile=profile,
        )
    )
    started = time.monotonic()
    artifact = generate_pixel_avatar(
        generate_request,
        client=client,
        style=style,  # type: ignore[arg-type]
    )
    generate_seconds = time.monotonic() - started

    out_dir.mkdir(parents=True, exist_ok=True)
    image_path = out_dir / f"{label}.png"
    image_path.write_bytes(artifact.png)
    record = {
        "label": label,
        "sourcePhotos": [str(photo) for photo in photos],
        "species": profile.get("species"),
        "styleProfileId": profile.get("styleProfileId"),
        "analyzeSeconds": round(analyze_seconds, 1),
        "generateSeconds": round(generate_seconds, 1),
        "artifactPath": str(image_path),
        "artifactSha256": artifact.sha256,
        "artifactWidth": artifact.width,
        "artifactHeight": artifact.height,
        "audit": artifact.audit.to_wire() if hasattr(artifact.audit, "to_wire") else str(artifact.audit),
    }
    print(
        f"[{label}] species={record['species']} "
        f"分析 {record['analyzeSeconds']}s / 生成 {record['generateSeconds']}s "
        f"→ {artifact.width}x{artifact.height}  {image_path}"
    )
    return record


def main() -> None:
    parser = argparse.ArgumentParser(description="L1 真实照片验收（pixel / schema 3 路线）")
    parser.add_argument("--label", required=True, help="样本标签，如 短毛猫")
    parser.add_argument("--photo", action="append", required=True, help="照片路径，可重复")
    parser.add_argument(
        "--out-dir",
        default=str(REPO_ROOT / "output" / "L1验收"),
        help="产物输出目录（默认 output/L1验收，已被 .gitignore 覆盖）",
    )
    args = parser.parse_args()

    photos = [Path(item) for item in args.photo]
    for photo in photos:
        if not photo.is_file():
            raise SystemExit(f"照片不存在：{photo}")

    config = load_config()
    client = Lk888Client(config, httpx.Client())
    style = load_pixel_style_pack(PIXEL_STYLE_V1_ID)
    out_dir = Path(args.out_dir)

    record = run_one(
        label=args.label,
        photos=photos,
        client=client,
        style=style,
        out_dir=out_dir,
    )
    records_path = out_dir / "验收记录.jsonl"
    with records_path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(record, ensure_ascii=False) + "\n")
    print(f"[{args.label}] 记录已追加到 {records_path}")


if __name__ == "__main__":
    main()
