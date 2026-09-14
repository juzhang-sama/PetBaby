# -*- coding: utf-8 -*-
"""绿幕视频 → 可安装的 schema 7 运行时包（一支 zip）。

抠像 → 四判据验收 → 打包 schema 7（WebP）→ zip。对应服务 step `packFrameSequence`。

上游的「母版 → 绿幕首帧 → 视频」是另一个 step（`generateMotionSource`）：
那一步要花真钱（12s 标准 480p = 5.02 算力），所以单独切出来按「钱」分开重试；
**本模块不碰任何 API，跑一次 0 算力。**

产物都落在 `out_dir` 下，目录名与 `scripts/一键出宠.py` 一致，方便对着旧产物比对：

    out_dir/04-抠像/          frames/*.png + 抠像参数.json（审计留档）
    out_dir/05-验收/          验收报告.json + 5 张证据图
    out_dir/10-运行时包/       manifest.json + frames/<action>/*.webp
    out_dir/<petId>.zip       ← 交付物（= 10-运行时包 整棵树的 zip）

⚠️ **验收 FAIL 也照样出包。** 老王定的上线形态是「后台任务 + 自动重试 + 人工确认」——
四判据是**排雷**不是闸门（机械指标 FAIL 但证据图干净 = 误报，这是验收纪律写明的）。
所以本函数把 `overall_passed` / `failed_criteria` 交回去，由上层决定要不要给用户装。
⚠️ **人工确认要看证据图，而 zip 里只有运行时包**（manifest + 帧）。
「证据图怎么送到客户端」是第 5 片契约扩展要定的事（一个 job 只能有一个 artifact），
本模块不做假设、也不把非运行时文件塞进 zip。

`out_dir` 必须是**空目录或不存在**：中间产物上百个文件，原地重跑要批量删除，
会被本机的批量删除守卫拦下来。调用方给每个 job 一个全新的 scratch 目录。
"""
from __future__ import annotations

import zipfile
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from . import packing
from ._paths import rel_to
from .acceptance import AcceptanceResult, accept_frames
from .matting import MatteResult, matte_video
from .packing import WEBP_QUALITY

MATTE_DIR = "04-抠像"
ACCEPTANCE_DIR = "05-验收"
PACKAGE_DIR = "10-运行时包"
MANIFEST_NAME = "manifest.json"

# zip 条目时间戳固定成常数：**同样的内容必须产同样的字节**。
# zip 默认把源文件 mtime 写进条目头，那样「重跑一次比对 zip」这种验证根本做不了。
_ZIP_TIMESTAMP = (1980, 1, 1, 0, 0, 0)


@dataclass(frozen=True)
class FrameSequenceBuild:
    out_dir: Path
    zip_path: Path
    zip_bytes: int
    package_dir: Path
    manifest_path: Path
    manifest_sha256: str
    frame_count: int
    frame_duration_ms: int
    frame_format: str
    acceptance: AcceptanceResult
    matte: MatteResult

    @property
    def overall_passed(self) -> bool:
        return self.acceptance.overall_passed

    @property
    def failed_criteria(self) -> tuple[str, ...]:
        return self.acceptance.failed_criteria


def zip_package(package_dir: Path, zip_path: Path) -> tuple[Path, int]:
    """把运行时包整棵树打成一支 zip（条目名相对包根，解开就是 `<petId>/` 的内容）。

    条目按路径排序 + 时间戳固定 → 同一份内容永远同一串字节。
    """
    package_dir = Path(package_dir)
    zip_path = Path(zip_path)
    if not (package_dir / MANIFEST_NAME).is_file():
        raise ValueError(f"package has no {MANIFEST_NAME}: {package_dir}")
    entries = sorted(path for path in package_dir.rglob("*") if path.is_file())
    if not entries:
        raise ValueError(f"package directory is empty: {package_dir}")

    zip_path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(zip_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for path in entries:
            info = zipfile.ZipInfo(
                path.relative_to(package_dir).as_posix(), date_time=_ZIP_TIMESTAMP
            )
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            archive.writestr(info, path.read_bytes())
    return zip_path, zip_path.stat().st_size


def build_frame_sequence(
    video: Path,
    out_dir: Path,
    *,
    pet_id: str,
    display_name: str,
    species: str = "cat",
    variant_id: str = "combo-loop-v1",
    fps: float = 24.0,
    frame_duration_ms: int = packing.FRAME_MS,
    webp_quality: int = WEBP_QUALITY,
    color_match: Path | None = None,
    path_base: Path | None = None,
    log: Callable[[str], None] = print,
) -> FrameSequenceBuild:
    """跑完 `packFrameSequence` 这一步，返回产物位置与验收结论。

    只接受空目录（见模块开头的说明）。不抛验收 FAIL —— FAIL 是结论不是异常。
    """
    video = Path(video)
    out_dir = Path(out_dir)
    if not video.is_file():
        raise ValueError(f"video does not exist: {video}")
    if out_dir.exists() and any(out_dir.iterdir()):
        raise ValueError(
            f"output directory must be empty (give each job a fresh scratch dir): {out_dir}"
        )
    out_dir.mkdir(parents=True, exist_ok=True)

    log(f"[1/4] 抠像  {video.name}  （fps={fps} 帧时长={frame_duration_ms}ms）")
    matte = matte_video(
        video,
        out_dir / MATTE_DIR,
        fps=fps,
        frame_duration_ms=frame_duration_ms,
        color_match=color_match,
        path_base=path_base,
        log=log,
    )

    log("[2/4] 四项验收")
    acceptance = accept_frames(
        matte.frames_dir, out_dir / ACCEPTANCE_DIR, path_base=path_base, log=log
    )

    log("[3/4] 打包 schema 7（webp）")
    packed = packing.pack_frame_sequence(
        frames_dir=matte.frames_dir,
        out_dir=out_dir / PACKAGE_DIR,
        pet_id=pet_id,
        display_name=display_name,
        species=species,
        variant_id=variant_id,
        frame_duration_ms=frame_duration_ms,
        frame_format="webp",
        webp_quality=webp_quality,
    )

    log("[4/4] 打 zip")
    zip_path, zip_bytes = zip_package(packed.out_dir, out_dir / f"{pet_id}.zip")
    png_bytes = sum(path.stat().st_size for path in matte.frames_dir.glob("*.png"))
    log(f"[产物] {rel_to(zip_path, path_base)}  {zip_bytes / 1024 / 1024:.1f} MB"
        f"  （PNG 源帧 {png_bytes / 1024 / 1024:.1f} MB / {packed.frame_count} 帧）")

    return FrameSequenceBuild(
        out_dir=out_dir,
        zip_path=zip_path,
        zip_bytes=zip_bytes,
        package_dir=packed.out_dir,
        manifest_path=packed.manifest_path,
        manifest_sha256=packed.manifest_sha256,
        frame_count=packed.frame_count,
        frame_duration_ms=packed.frame_duration_ms,
        frame_format=packed.frame_format,
        acceptance=acceptance,
        matte=matte,
    )
