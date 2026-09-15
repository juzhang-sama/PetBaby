"""对比视频档位（Mini / 标准）在同一链路上的表现与花费。

**为什么只跑一支**：`generateMotionSource` 一次会跑四支视频（idle + 3 个动作），
拿它试档位等于用 4 倍的钱做实验。这里只重跑 **idle** 一支，而且**复用已有的母版
与提示词**（`render_loop_prompt()` 零替换位）—— 变量只剩「模型 + 档位」，对比才干净。

⚠️ **首帧必须走生产同一条免费阶梯**（`_converge_first_frame`，0.90→0.85→0.80→0.75，
两个余量都 ≥5% 才放行）。**别手写死某一档**：2026-09-15 实测踩过 —— 脚本原先硬编码
`scale-90`，而 pet-4750（长毛白猫）生产真正收敛的是 **scale-80**
（scale-90 的右余量是 **-4.7%**，生产闸口根本不会放行）。
拿 0.90 的首帧去对比，等于把「档位差异」和「取景差异（猫大 12.5%）」混在一起。

⚠️ 输出落到**新的 providerSessionId 目录**，绝不覆盖 scratch 里已有的 mp4：
那些是「重试不重付」的资产（`_has_video` 命中就跳过，等于白捡）。

用法：
    python scripts/poc_对比视频档位.py --version Mini --resolution 480p --duration 12
    python scripts/poc_对比视频档位.py --version 标准 --resolution 480p --duration 12
    # 想钉死某一张首帧（例如复现历史那支）：
    python scripts/poc_对比视频档位.py --version 标准 --first-frame <绿幕首帧.png>
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SERVICE = ROOT / "services" / "appearance-generation"
sys.path.insert(0, str(SERVICE / "src"))

import httpx  # noqa: E402

from photo_avatar_backend import frame_pipeline as fp  # noqa: E402
from photo_avatar_backend.config import BackendConfig  # noqa: E402
from photo_avatar_backend.lk888_client import Lk888Client  # noqa: E402

ENV_PATH = SERVICE / ".env"
STATE_DIR = SERVICE / "output" / "photo-avatar-backend"
# pet-4750（毛毛毛毛）那次的 gpt-image-2 透明母版（2048×2048）。
# 复用它 + 重跑生产那条取景阶梯，才能拿到「生产真正会用」的首帧 ——
# 首帧若手写死档位，换档位的对比里就混进了取景差异。
DEFAULT_MASTER = (
    STATE_DIR
    / "scratch"
    / "frf34903fef6b2a24f5d8c57e43c8969931c856ed937b22619a80aa77c58a89df2"
    / "motion-source"
    / "attempt-1"
    / "00-母版"
    / "母版-136680314.png"
)


def load_env(path: Path) -> dict[str, str]:
    env: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            key, value = line.split("=", 1)
            env[key.strip()] = value.strip()
    return env


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", default="Mini", choices=["Mini", "快速", "标准"])
    parser.add_argument("--resolution", default="480p", choices=["480p", "720p", "1080p", "4K"])
    parser.add_argument("--duration", default="12")
    parser.add_argument(
        "--master", default=None, help="母版 PNG；默认用 pet-4750 的 母版-136680314.png"
    )
    parser.add_argument(
        "--first-frame",
        default=None,
        help="直接指定首帧，跳过取景阶梯（只在复现某支历史视频时才用）",
    )
    parser.add_argument(
        "--out-id",
        default=None,
        help="scratch 目录名（= providerSessionId）。默认按档位生成，别指向已有目录",
    )
    args = parser.parse_args()

    out_id = args.out_id or f"verify-{args.version}-{args.resolution}-{args.duration}s"
    out_path = fp.motion_source_path(STATE_DIR, out_id)
    if out_path.is_file():
        print(f"⚠️ {out_path} 已存在，会被覆盖。换个 --out-id 或先删掉它。", file=sys.stderr)
        return 2

    # 首帧留在 out_id 自己的 scratch 目录里，布局与生产一致（motion-source/attempt-1/01-首帧）。
    work_dir = fp.scratch_dir(STATE_DIR, out_id) / "motion-source" / "attempt-1"

    first_frame_scale: float | None = None
    first_frame_left: float | None = None
    first_frame_right: float | None = None
    if args.first_frame:
        first_frame = Path(args.first_frame)
        if not first_frame.is_file():
            print(f"首帧不存在：{first_frame}", file=sys.stderr)
            return 2
    else:
        master = Path(args.master) if args.master else DEFAULT_MASTER
        if not master.is_file():
            print(f"母版不存在：{master}", file=sys.stderr)
            return 2
        print(f"[取景] 按生产阶梯收敛首帧（0 算力，本地） 母版={master.name}")
        fitted = fp._converge_first_frame(  # noqa: SLF001 —— 探针就是要和生产走同一条阶梯
            master, work_dir=work_dir, state_dir=STATE_DIR, log=print
        )
        first_frame = fitted.frame_png
        first_frame_scale = fitted.scale
        first_frame_left = fitted.left_margin
        first_frame_right = fitted.right_margin_at_full_swing

    env = load_env(ENV_PATH)
    config = BackendConfig(
        lk888_api_key=env["LK888_API_KEY"],
        backend_token=env["PHOTO_AVATAR_BACKEND_TOKEN"],
        state_dir=STATE_DIR,
    )

    # `_generate_video` 从模块全局读这几个常量（不是在参数里），所以改模块属性即可。
    fp.VIDEO_VERSION = args.version
    fp.VIDEO_RESOLUTION = args.resolution
    fp.VIDEO_DURATION = args.duration

    print(
        f"模型={config.video_model}  档位={args.version}  {args.resolution}  {args.duration}s\n"
        f"首帧={first_frame}\n输出={out_path}\n"
    )

    started = time.time()
    # ⚠️ 必须 `trust_env=False`：本机开着系统 HTTP 代理，httpx 默认会吃它，
    # 表现为轮询时报 `CERTIFICATE_VERIFY_FAILED: Hostname mismatch ... api.lk888.ai`
    # —— 任务其实**已经提交成功**，只是后半程挂了（2026-09-15 实测踩到，白捡回 task 137614039）。
    client = Lk888Client(config, httpx.Client(trust_env=False))
    task_id = fp._generate_video(  # noqa: SLF001 —— 探针就是要打这一层
        client=client,
        first_frame=first_frame,
        video_path=out_path,
        prompt=fp.render_loop_prompt(),
        report_task_id=None,
        label=f"idle[{args.version}]",
        log=print,
    )
    elapsed = time.time() - started

    # 花费只信平台自己报的 cost（按秒计费下 = 秒数 × 档位价）。
    cost = None
    try:
        with httpx.Client(trust_env=False, timeout=30) as probe:
            resp = probe.get(
                "https://api.lk888.ai/v1/skills/task-status",
                params={"task_id": task_id},
                headers={"Authorization": "Bearer " + config.lk888_api_key},
            )
            payload = resp.json()
            cost = payload.get("cost")
            duration_seconds = payload.get("duration_seconds")
    except Exception as error:  # noqa: BLE001 —— 查费用失败不该把已生成的结果丢掉
        payload = {"error": repr(error)}
        duration_seconds = None

    size_kb = out_path.stat().st_size // 1024 if out_path.is_file() else 0
    summary = {
        "model": config.video_model,
        "version": args.version,
        "resolution": args.resolution,
        "duration": args.duration,
        # 首帧出处必须记下来 —— 换档位对比要成立，两支视频的首帧得是同一张。
        "firstFramePath": str(first_frame),
        "firstFrameScale": first_frame_scale,
        "firstFrameLeftMargin": first_frame_left,
        "firstFrameRightMarginAtFullSwing": first_frame_right,
        "taskId": task_id,
        "cost": cost,
        "durationSeconds": duration_seconds,
        "elapsedSeconds": round(elapsed, 1),
        "outputKB": size_kb,
        "outputPath": str(out_path),
        "statusPayload": payload,
    }
    print("\n" + json.dumps(summary, ensure_ascii=False, indent=2))

    summary_path = SERVICE / "output" / "photo-avatar-backend" / f"档位对比-{out_id}.json"
    summary_path.write_text(json.dumps(summary, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"\n摘要已写：{summary_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
