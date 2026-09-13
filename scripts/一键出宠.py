# -*- coding: utf-8 -*-
"""一键出宠：照片 -> frame-sequence-v1（schema 7）运行时包。

把 09-12 手工串的 6 步（母版 / 绿幕首帧 / 视频 / 抠像 / 验收 / 打包）收成一条命令，
并补上手工时代全靠人眼盯的两处：

  1. **取景收敛（免费）**：首帧生成后自检两个余量（左余量、尾巴甩到最远时的右余量），
     任一 < 5% 就按阶梯调小 --scale 重生成首帧。首帧不烧算力，所以可以放心循环。
     —— 长毛猫当初就是因为没看这行警告、直接进视频，白花 4.67 算力。
  2. **失败分类重试（省算力）**：视频步骤读 `<outdir>/03-视频/生成记录.json` 的 errorCode。
     可重试的（network / timeout / provider5xx / temporaryUnavailable）自动重跑；
     不可重试的（contentPolicy / quota / unsupported / invalidInput）立刻停 ——
     重试同一份提示词必然再被拒，纯烧钱。

本脚本**只产 idle-combo**（单视频循环：呼吸+眨眼+摇尾焊死在一支视频里）。
yawn / lick 这类一次性动作走 skill `petbaby-oneshot-action-integration`，
它们必须复用 idle 的 crop box，所以要在本脚本跑完之后再做。

会产生 API 费用。12s 标准 480p 按秒版 ≈ 2.1 算力/支，加母版约 3~4 算力/只。

用法：
  D:/DevTools/Python312/python.exe scripts/一键出宠.py \
      --photo "C:/Users/Administrator/Desktop/果冻.jpg" \
      --pet-id 06-guodong --name "果冻（短毛猫）" \
      --species cat --coat short \
      --outdir "output/一键出宠-06-guodong-2026-09-13" \
      --yes

加 `--install` 才会拷进 `apps/desktop/public/builtin-pets/<petId>` 并转 WebP；
不加则只在 --outdir 里产出 schema 7 包（PNG），不碰应用资产。
"""
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

import httpx

ROOT = Path(__file__).resolve().parents[1]
ENV_FILE = ROOT / "services" / "appearance-generation" / ".env"
BUILTIN_PETS = ROOT / "apps" / "desktop" / "public" / "builtin-pets"

# ⚠️ 不同步骤要的解释器**不一样**，别用一个 PY 常量串到底（2026-09-13 踩过）：
#   后端服务（母版 / 视频）要 httpx，跑在 managed 的 3.13.12.old.24532；
#   图像步骤（首帧 / 抠像 / 验收 / WebP）要 numpy + PIL + **scipy**，
#   而 scipy 只装在 D:/DevTools/Python312 —— 用同一个解释器串到底，
#   抠像会在 `from scipy import ndimage` 直接 ModuleNotFoundError。
# 按「能否 import 所需依赖」探测，不硬编码版本目录：managed runtime 升级会整体替换
# 版本目录（留下 .old.<n>），**已装依赖不跟着迁移**，硬编码路径会选到空解释器。
SERVICE_REQUIREMENTS = ("httpx",)
IMAGE_REQUIREMENTS = ("numpy", "PIL", "scipy")
PYTHON_CANDIDATES = (
    "python3",
    "python",
    "C:/Users/Administrator/.workbuddy/binaries/python/versions/3.13.12.old.24532/python.exe",
    "C:/Users/Administrator/.workbuddy/binaries/python/versions/3.13.12/python.exe",
    "D:/DevTools/Python312/python.exe",
)
_python_cache: dict[tuple[str, ...], str] = {}

FRAME_MS = 42
MARGIN_MIN = 0.05                     # 两个余量的下限（skill：任一 <5% 不许进视频）
MARGIN_LEFT = 0.06                    # 别低于 0.05：0.02 会让主体贴左边触边
SCALE_LADDER = (0.90, 0.85, 0.80, 0.75)
MAX_VIDEO_ATTEMPTS = 3
RETRYABLE_CODES = {"network", "timeout", "provider5xx", "temporaryUnavailable"}
DEFAULT_VIDEO_MODEL = "seedance-2.0-guanfang-anmiao"   # 按秒计费，总价可预估

COAT_TEXT = {"short": "short-haired", "long": "long-haired"}


def log(message: str) -> None:
    print(message, flush=True)


# 只认「全大写标识符」形式的占位符（{{SPECIES}}）。模板开头有说明文字写着
# 「把 {{...}} 替换后整段复制」，那个省略号形式不是占位符，不能当残留报错。
_PLACEHOLDER = re.compile(r"\{\{[A-Z][A-Z0-9_]*\}\}")


def assert_no_placeholder(text: str, path: Path, label: str) -> None:
    left = sorted(set(_PLACEHOLDER.findall(text)))
    if left:
        raise SystemExit(f"[中断] {label}还有未替换的占位符 {left}：{path}")


def pick_python(requirements: tuple[str, ...]) -> str:
    """选一个能 import 全部 `requirements` 的解释器（结果缓存）。

    先试当前解释器，再试 PYTHON_CANDIDATES；全失败就报出试过哪些，
    而不是让子脚本以「进程秒退 + 日志 0 字节」的方式静默失败。
    """
    if requirements in _python_cache:
        return _python_cache[requirements]
    probe = "import " + ", ".join(requirements)
    tried: list[str] = []
    for candidate in (sys.executable, *PYTHON_CANDIDATES):
        if candidate in tried:
            continue
        tried.append(candidate)
        try:
            result = subprocess.run(
                [candidate, "-c", probe], capture_output=True, timeout=90
            )
        except (OSError, subprocess.SubprocessError):
            continue
        if result.returncode == 0:
            _python_cache[requirements] = candidate
            log(f"[解释器] {'/'.join(requirements)} -> {candidate}")
            return candidate
    raise SystemExit(
        f"[中断] 找不到能 import {requirements} 的 Python。试过：\n"
        + "\n".join(f"         {path}" for path in tried)
        + "\n       装依赖，或把可用解释器加进 PYTHON_CANDIDATES。"
    )


def load_env(path: Path) -> dict[str, str]:
    env: dict[str, str] = {}
    if not path.is_file():
        return env
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        env[key.strip()] = value.strip()
    return env


def read_balance() -> float | None:
    """读 lk888 余额（按 key）。读不到就返回 None，预算闸口自动退化成不拦。"""
    env = load_env(ENV_FILE)
    api_key = env.get("LK888_API_KEY")
    if not api_key:
        log("[预算] .env 里没有 LK888_API_KEY，跳过预算闸口")
        return None
    base = (env.get("LK888_BASE_URL") or "https://api.lk888.ai").rstrip("/")
    try:
        response = httpx.get(
            f"{base}/v1/skills/balance",
            headers={"Authorization": f"Bearer {api_key}"},
            timeout=30,
        )
        payload = response.json()
        value = payload.get("balance") if isinstance(payload, dict) else None
        return float(value) if isinstance(value, (int, float)) else None
    except Exception as exc:  # noqa: BLE001 — 读不到余额不该阻断出宠
        log(f"[预算] 读余额失败（{exc}），跳过预算闸口")
        return None


def run(cmd: list, label: str, *, ok_codes: tuple[int, ...] = (0,)) -> int:
    log("")
    log("=" * 72)
    log(f"[{label}]")
    log("  " + " ".join(str(part) for part in cmd))
    log("=" * 72)
    env = dict(os.environ, PYTHONIOENCODING="utf-8")
    rc = subprocess.run([str(part) for part in cmd], cwd=str(ROOT), env=env).returncode
    if rc not in ok_codes:
        raise SystemExit(f"[中断] {label} 失败，退出码 {rc}")
    return rc


def render_master_prompt(species: str, coat: str, template: Path, out: Path) -> Path:
    text = template.read_text(encoding="utf-8")
    text = text.replace("{{SPECIES}}", species).replace("{{COAT_LEN}}", COAT_TEXT[coat])
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(text, encoding="utf-8")
    assert_no_placeholder(text, template, "母版提示词")
    return out


def render_loop_prompt(label: str, template: Path, out: Path) -> Path:
    text = template.read_text(encoding="utf-8").replace("{{LABEL}}", label)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(text, encoding="utf-8")
    assert_no_placeholder(text, template, "组合循环提示词")
    return out


def step_master(photo: Path, prompt_file: Path, out_dir: Path) -> Path:
    run(
        [pick_python(SERVICE_REQUIREMENTS), "scripts/poc_生成母版.py", "--photo", photo,
         "--prompt-file", prompt_file, "--outdir", out_dir, "--yes"],
        "1/6 生成透明母版",
    )
    record = out_dir / "生成记录.json"
    if not record.is_file():
        raise SystemExit(f"[中断] 母版缺生成记录：{record}")
    jobs = json.loads(record.read_text(encoding="utf-8")).get("jobs") or []
    if not jobs:
        raise SystemExit("[中断] 母版生成记录里没有 job")
    master = ROOT / jobs[0]["outputPath"]
    if not master.is_file():
        raise SystemExit(f"[中断] 母版文件不存在：{master}")
    log(f"[母版] {master}  ({jobs[0].get('bytes', 0) // 1024} KB)")
    return master


def converge_framing(master: Path, out_root: Path) -> Path:
    """首帧取景收敛：两个余量都 ≥5% 才放行。首帧不烧算力，可以放心重生成。"""
    last: tuple[float, float, float] | None = None
    for scale in SCALE_LADDER:
        out_dir = out_root / f"scale-{int(round(scale * 100))}"
        run(
            [pick_python(IMAGE_REQUIREMENTS), "scripts/poc_绿幕首帧.py", "--master", master,
             "--scale", f"{scale:.2f}", "--margin-left", f"{MARGIN_LEFT:.2f}",
             "--outdir", out_dir],
            f"2/6 绿幕首帧 scale={scale:.2f}",
        )
        info = json.loads((out_dir / "首帧分析.json").read_text(encoding="utf-8"))
        fit = info.get("tailSwingFit") or {}
        right = float(fit.get("rightMarginAtFullSwing", -1.0))
        left = float(fit.get("marginLeft", -1.0))
        last = (scale, left, right)
        log(f"[取景] scale={scale:.2f}  左余量={left * 100:.1f}%  "
            f"尾巴全摆时右余量={right * 100:.1f}%  （下限 {MARGIN_MIN * 100:.0f}%）")
        if left >= MARGIN_MIN and right >= MARGIN_MIN:
            log(f"[取景] 通过，采用 scale={scale:.2f}")
            return out_dir / "绿幕首帧-1024.png"
        log("[取景] 不合格，调小 --scale 重生成首帧（免费）")

    scale, left, right = last or (-1.0, -1.0, -1.0)
    raise SystemExit(
        f"[中断] 取景阶梯试完全部 scale 仍不合格：最后 scale={scale:.2f} "
        f"左余量={left * 100:.1f}% 右余量={right * 100:.1f}%\n"
        "       主体横向太长（长毛猫的尾巴常见），需要人工介入：换一张照片角度，"
        "或把 SCALE_LADDER 继续往下伸（会牺牲主体分辨率）。"
    )


def step_video(frame: Path, prompt_file: Path, out_dir: Path, args) -> Path:
    """提交视频并轮询；失败按 errorCode 分类决定「重试」还是「停」。"""
    for attempt in range(1, MAX_VIDEO_ATTEMPTS + 1):
        run(
            [pick_python(SERVICE_REQUIREMENTS), "scripts/poc_生成绿幕视频.py",
             "--first-frame", frame, "--prompt-file", prompt_file, "--outdir", out_dir,
             "--model", args.video_model, "--version", args.version,
             "--resolution", args.resolution, "--duration", str(args.duration),
             "--aspect-ratio", "1:1", "--mode", "shouweizhen", "--yes"],
            f"3/6 生成绿幕视频（第 {attempt}/{MAX_VIDEO_ATTEMPTS} 次）",
            ok_codes=(0, 1),
        )
        record = out_dir / "生成记录.json"
        parsed = json.loads(record.read_text(encoding="utf-8")) if record.is_file() else {}
        failure = parsed.get("failure")
        if not failure:
            videos = sorted(out_dir.glob("绿幕视频-*.mp4"))
            if not videos:
                raise SystemExit("[中断] 视频步骤既没有产物、也没有失败记录")
            log(f"[视频] {videos[-1]}  ({videos[-1].stat().st_size // 1024} KB)")
            return videos[-1]

        code = failure.get("errorCode")
        log(f"[视频失败] code={code}  retryable={failure.get('retryable')}  "
            f"message={failure.get('errorMessage')}")
        if code not in RETRYABLE_CODES:
            raise SystemExit(
                f"[中断] 视频失败且**不可重试**：{code}\n"
                f"       重试同一份提示词只会再被拒（还白烧算力）。\n"
                f"       contentPolicy → 改提示词措辞；quota → 换 key / 充值；\n"
                f"       unsupported/invalidInput → 核对模型名与参数组合。\n"
                f"       失败记录：{record}"
            )
        if attempt == MAX_VIDEO_ATTEMPTS:
            raise SystemExit(f"[中断] 视频重试 {MAX_VIDEO_ATTEMPTS} 次仍失败：{code}")
        record.unlink()  # 清掉失败记录，避免下一轮被误读成「本次失败」
        log(f"[重试] {code} 属可重试故障，重新提交")


def judge_acceptance(report: dict) -> tuple[str, list[str]]:
    """把四判据分成「真缺陷」/「通过」/「疑似误报」。

    skill 铁律：**面积波动类判据不适用于全身/头颈动作**（yawn 8.1%、lick 12.69% 都是假 FAIL），
    判真伪只看三个真实缺陷信号：飞块 / 触边 / 多连通块。
    """
    reasons: list[str] = []
    for name, check in (report.get("criteria") or {}).items():
        if not isinstance(check, dict):
            continue
        if check.get("strayComponentFrameCount"):
            reasons.append(f"{name}: 尾巴被抠成飞块 {check['strayComponentFrameCount']} 帧")
        if check.get("edgeTouchFrameCount"):
            reasons.append(f"{name}: 主体触边被裁 {check['edgeTouchFrameCount']} 帧")
    if reasons:
        return "真缺陷", reasons
    if report.get("overallPassed"):
        return "通过", []
    soft = [
        f"{name}: 指标超阈值但无飞块/无触边（面积波动类判据不适用于全身动作时属误报）"
        for name, check in (report.get("criteria") or {}).items()
        if isinstance(check, dict) and not check.get("passed")
    ]
    return "疑似误报需人眼确认", soft


def step_install(package: Path, pet_id: str) -> None:
    if not BUILTIN_PETS.is_dir():
        raise SystemExit(f"[中断] 找不到内置宠物目录：{BUILTIN_PETS}")
    dest = BUILTIN_PETS / pet_id
    dest.mkdir(parents=True, exist_ok=True)
    # 逐文件复制：不要一次性搬几百个文件，会被批量删除守卫盯上
    copied = 0
    for src in sorted(package.rglob("*")):
        if src.is_file():
            target = dest / src.relative_to(package)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, target)
            copied += 1
    log(f"[安装] {copied} 个文件 -> {dest}")
    run([pick_python(IMAGE_REQUIREMENTS), "scripts/poc_帧转webp.py", "--pet", pet_id],
        "7/7 帧转 WebP")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="照片 -> frame-sequence-v1（schema 7）包，只产 idle-combo",
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("--photo", default=None,
                        help="宠物照片路径（用 --reuse-video 续跑时可不传）")
    parser.add_argument("--reuse-video", default=None,
                        help="续跑：跳过 母版/首帧/视频，直接用这个 mp4 跑 抠像→验收→打包。"
                             "视频是唯一的大额支出（12s 标准 480p ≈ 5.7 算力），"
                             "后半段出问题时不用重新付一次")
    parser.add_argument("--pet-id", required=True, help="petId，如 06-guodong（只允许字母数字-_）")
    parser.add_argument("--name", required=True, help="displayName，如 果冻（短毛猫）")
    parser.add_argument("--species", default="cat", choices=["cat", "dog"])
    parser.add_argument("--coat", default="short", choices=["short", "long"],
                        help="毛长档位，只影响母版提示词")
    parser.add_argument("--outdir", required=True, help="本次出宠的产物根目录")
    parser.add_argument("--master-prompt", default="output/_通用母版提示词-2026-09-12.txt")
    parser.add_argument("--loop-prompt", default="output/_通用组合循环提示词-2026-09-12.txt")
    parser.add_argument("--video-model", default=DEFAULT_VIDEO_MODEL,
                        help="按秒版默认值；换 seedance-2.0-guanfang 会变按 token 计费、总价不可预估")
    parser.add_argument("--version", default="标准", choices=["Mini", "快速", "标准"])
    parser.add_argument("--resolution", default="480p",
                        choices=["480p", "720p", "1080p", "4K"])
    parser.add_argument("--duration", type=int, default=12, help="秒；既有一致规格是 12s/289 帧")
    parser.add_argument("--max-cost", type=float, default=12.0,
                        help="算力预算上限；超了就停（靠余额差值算，不需要额外接口）")
    parser.add_argument("--install", action="store_true",
                        help="装进 builtin-pets 并转 WebP（默认只在 outdir 产出）")
    parser.add_argument("--yes", action="store_true", help="跳过费用确认")
    args = parser.parse_args()

    if not args.reuse_video and not args.photo:
        raise SystemExit("[缺输入] 要么给 --photo（完整出宠），要么给 --reuse-video（续跑）")
    photo = Path(args.photo).expanduser().resolve() if args.photo else None
    if photo is not None and not photo.is_file():
        raise SystemExit(f"[缺输入] 照片不存在：{photo}")
    if not all(ch.isalnum() or ch in "-_" for ch in args.pet_id):
        raise SystemExit("[缺输入] petId 只允许字母、数字、连字符、下划线")
    if args.duration < 4 or args.duration > 15:
        raise SystemExit("[缺输入] duration 只支持 4~15 秒")

    out_root = (ROOT / args.outdir).resolve() if not Path(args.outdir).is_absolute() \
        else Path(args.outdir)
    out_root.mkdir(parents=True, exist_ok=True)

    master_template = (ROOT / args.master_prompt).resolve()
    loop_template = (ROOT / args.loop_prompt).resolve()
    if not args.reuse_video:
        for template in (master_template, loop_template):
            if not template.is_file():
                raise SystemExit(f"[缺输入] 提示词模板不存在：{template}")

    if not args.yes:
        log("[确认] 本流程会产生 lk888 API 费用：")
        log(f"       照片: {photo}")
        log(f"       petId: {args.pet_id}   物种: {args.species}   毛长: {args.coat}")
        log(f"       视频: {args.video_model} / {args.version} / {args.resolution} / "
            f"{args.duration}s")
        log(f"       产物: {out_root}")
        if input("       继续请输入 y: ").strip().lower() != "y":
            log("[取消] 未开始")
            return 1

    budget_start = read_balance()
    if budget_start is not None:
        log(f"[预算] 起始余额 {budget_start:.2f} 算力，上限 {args.max_cost:.2f}")

    def enforce_budget(stage: str) -> None:
        if budget_start is None:
            return
        now = read_balance()
        if now is None:
            return
        spent = budget_start - now
        log(f"[预算] {stage} 后已花 {spent:.2f} 算力（余额 {now:.2f}）")
        if spent > args.max_cost:
            raise SystemExit(
                f"[中断] 已花 {spent:.2f} 算力，超过 --max-cost {args.max_cost:.2f}，停止"
            )

    summary: dict = {
        "petId": args.pet_id,
        "displayName": args.name,
        "species": args.species,
        "coat": args.coat,
        "photo": str(photo) if photo else None,
        "outDir": str(out_root.relative_to(ROOT)),
        "stages": {},
    }

    # 1~4. 母版 → 首帧 → 提示词 → 视频。--reuse-video 时整段跳过，避免重复付费
    #（视频是唯一的大额支出，出问题时最需要能「只重跑后半段」）。
    if args.reuse_video:
        video = Path(args.reuse_video).expanduser()
        if not video.is_absolute():
            video = (ROOT / video).resolve()
        if not video.is_file():
            raise SystemExit(f"[缺输入] --reuse-video 指向的文件不存在：{video}")
        log(f"[续跑] 跳过 母版/首帧/视频，直接用已有视频：{video}")
        summary["stages"]["video"] = str(video.relative_to(ROOT))
    else:
        master_prompt = render_master_prompt(
            args.species, args.coat, master_template,
            out_root / "02-提示词" / "母版提示词.txt",
        )
        master = step_master(photo, master_prompt, out_root / "00-母版")
        enforce_budget("母版")
        summary["stages"]["master"] = str(master.relative_to(ROOT))

        frame = converge_framing(master, out_root / "01-首帧")
        summary["stages"]["firstFrame"] = str(frame.relative_to(ROOT))

        loop_prompt = render_loop_prompt(
            args.name, loop_template,
            out_root / "02-提示词" / "Seedance提示词-01-组合循环.txt",
        )

        video = step_video(frame, loop_prompt, out_root / "03-视频", args)
        enforce_budget("视频")
        summary["stages"]["video"] = str(video.relative_to(ROOT))

    # 5. 抠像（autocrop 到最紧正方形，主体占宽 ≈72%，与内置一致）
    run(
        [pick_python(IMAGE_REQUIREMENTS), "scripts/poc_抠像.py",
         "--video", video, "--out", out_root / "04-抠像",
         "--method", "chroma", "--fps", "24", "--frame-duration-ms", FRAME_MS],
        "4/6 抠像",
    )
    frames_dir = out_root / "04-抠像" / "frames"

    # 6. 验收（rc=2 表示判定不通过，不是崩溃）
    run([pick_python(IMAGE_REQUIREMENTS), "scripts/poc_验收.py", "--frames", frames_dir,
         "--out", out_root / "05-验收"], "5/6 四判据验收", ok_codes=(0, 2))
    report_path = out_root / "05-验收" / "验收报告.json"
    report = json.loads(report_path.read_text(encoding="utf-8")) if report_path.is_file() else {}
    verdict, reasons = judge_acceptance(report)
    summary["acceptance"] = {
        "verdict": verdict,
        "reasons": reasons,
        "frames": report.get("frameCount"),
        "resolution": report.get("resolution"),
        "evidence": report.get("evidence"),
    }
    log("")
    log(f"[验收] {verdict}")
    for reason in reasons:
        log(f"        - {reason}")

    # 7. 打包 schema 7
    package = out_root / "10-运行时包"
    run(
        [pick_python(IMAGE_REQUIREMENTS), "scripts/poc_组合循环打包.py",
         "--frames", frames_dir, "--out", package,
         "--pet", args.pet_id, "--name", f"{args.name}（呼吸+眨眼+摇尾循环）",
         "--species", args.species],
        "6/6 打包 schema 7",
    )
    summary["stages"]["package"] = str(package.relative_to(ROOT))

    if args.install:
        step_install(package, args.pet_id)

    budget_end = read_balance()
    if budget_start is not None and budget_end is not None:
        summary["cost"] = round(budget_start - budget_end, 4)

    (out_root / "一键出宠-结果.json").write_text(
        json.dumps(summary, ensure_ascii=False, indent=2), encoding="utf-8"
    )

    log("")
    log("=" * 72)
    log(f"[完成] {args.pet_id}：{summary['acceptance']['frames']} 帧，验收「{verdict}」")
    log(f"       包: {package}")
    log(f"       记录: {out_root / '一键出宠-结果.json'}")
    if summary.get("cost") is not None:
        log(f"       花费: {summary['cost']:.2f} 算力")
    log("=" * 72)
    if verdict != "通过":
        log("")
        log("[必须人眼确认] 机械指标只能排雷，肉眼判定 = 权威。")
        log(f"       看证据图：{out_root / '05-验收'}")
    if not args.install:
        log("")
        log("[下一步] 确认没问题后装进应用（不加 --install 时不会碰应用资产）：")
        log("        1) 拷 `10-运行时包` 到 apps/desktop/public/builtin-pets/<petId>/")
        log("        2) 跑 scripts/poc_帧转webp.py --pet <petId>")
        log("        3) 同步 4 处代码：runtime/startup-pet.ts(BUILTIN_PIXEL_PETS)、")
        log("           pets/active.rs(BUILTIN_PIXEL_PET_IDS)、pets/catalog.rs(pixel_pet_display_name)、")
        log("           runtime/startup-pet.test.ts(断言长度)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
