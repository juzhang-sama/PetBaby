# -*- coding: utf-8 -*-
"""一次性探针：真跑 gpt-4o 看照片，验收 `analyze_photo_facts`。

真值来自 `docs/验证记录/L1真实照片验收记录-2026-09-11.md` 第 3 节（那张表写明
每张真照片的判定物种，人眼判定里写明毛长）。四张覆盖 短/长 × 猫/犬 —— 正好是
「这套提示词会不会永远说 long」这个问题的答案。

只读桌面的四张源图，不上传、不落盘。跑完即删脚本。
"""
from __future__ import annotations

import hashlib
import io
import os
import sys
from pathlib import Path

import httpx
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
SERVICE = ROOT / "services" / "appearance-generation"
ENV_FILE = SERVICE / ".env"
DESKTOP = Path.home() / "Desktop"

sys.path.insert(0, str(SERVICE / "src"))

from photo_avatar_backend.config import BackendConfig  # noqa: E402
from photo_avatar_backend.contracts import FrameStepRequest, SourceImage  # noqa: E402
from photo_avatar_backend.frame_pipeline import analyze_photo_facts  # noqa: E402
from photo_avatar_backend.lk888_client import Lk888Client  # noqa: E402

# (标签, 文件, 真值物种, 真值毛长) —— 真值出处：L1 验收记录第 3 节 + 第 5 节人眼判定
CASES = [
    ("短毛猫（果冻）", DESKTOP / "果冻.jpg", "cat", "short"),
    ("长毛猫", DESKTOP / "长毛猫.jpeg", "cat", "long"),
    ("短毛犬（比格）", DESKTOP / "短毛犬.jpg", "dog", "short"),
    ("长毛犬（金毛）", DESKTOP / "金毛.webp", "dog", "long"),
]


def load_env(path: Path) -> dict[str, str]:
    env: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        env[key.strip()] = value.strip()
    return env


def balance(http: httpx.Client, base: str, key: str) -> str:
    try:
        resp = http.get(
            f"{base}/v1/skills/balance",
            headers={"Authorization": f"Bearer {key}"},
            timeout=30,
        )
        return resp.text[:200]
    except Exception as exc:  # noqa: BLE001 - 探针，余额查不到不该中断
        return f"<查不到: {exc}>"


def photo(path: Path) -> SourceImage:
    """契约要求 sourceImages 必须是 PNG（`_parse_source_image` 查 PNG 签名）。

    `.webp` 源图在真实链路里也是先转 PNG（L1 验收记录第 3 节已记这条）。
    """
    buffer = io.BytesIO()
    with Image.open(path) as image:
        image.convert("RGB").save(buffer, format="PNG")
    png = buffer.getvalue()
    with Image.open(io.BytesIO(png)) as image:
        width, height = image.size
    return SourceImage(
        source_id=path.stem,
        png=png,
        sha256=hashlib.sha256(png).hexdigest(),
        width=width,
        height=height,
    )


def request_for(source: SourceImage, species: str) -> FrameStepRequest:
    return FrameStepRequest(
        session_id="probe",
        revision=0,
        provider_session_id="probe",
        step="generateMotionSource",
        attempt=1,
        consent_version="probe",
        source_images=(source,),
        pet_id="probe-pet",
        display_name="探针",
        species=species,
    )


def main() -> None:
    env = dict(os.environ)
    env.update(load_env(ENV_FILE))
    config = BackendConfig.from_env(env)

    with httpx.Client() as http:
        before = balance(http, config.lk888_base_url, config.lk888_api_key)
        print(f"[余额·跑前] {before}\n")

        client = Lk888Client(config, http)
        passed = 0
        for label, path, expected_species, expected_coat in CASES:
            if not path.is_file():
                print(f"===== {label} =====\n   缺文件 {path}\n")
                continue
            print(f"===== {label} ===== ({path.name}，真值 {expected_species}/{expected_coat})")
            try:
                facts = analyze_photo_facts(
                    request_for(photo(path), expected_species),
                    client=client,
                    log=lambda line: print(f"   {line}"),
                )
            except Exception as exc:  # noqa: BLE001 - 探针要看到原始错误
                print(f"   !! {type(exc).__name__}: {exc}\n")
                continue
            ok_species = facts.species == expected_species
            ok_coat = facts.coat == expected_coat
            passed += ok_species and ok_coat
            print(
                f"   判据: 物种 {'PASS' if ok_species else 'FAIL'} / "
                f"毛长 {'PASS' if ok_coat else 'FAIL'}"
                f"   实际 species={facts.species!r} coat={facts.coat!r}\n"
            )

        print(f"合计 {passed}/{len(CASES)} 全对")
        print(f"[余额·跑后] {balance(http, config.lk888_base_url, config.lk888_api_key)}")


if __name__ == "__main__":
    main()
