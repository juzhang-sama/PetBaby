# -*- coding: utf-8 -*-
"""POC：把绿幕首帧 + Seedance 提示词提交给 lk888，拿回绿幕动作视频。

这一步替掉「手工把首帧和提示词粘到 Seedance 网页上」的动作，接在
`poc_生成母版.py` -> `poc_绿幕首帧.py` 之后，成为
「母版 -> 绿幕首帧 -> 绿幕视频 -> 抠像 -> 帧序列」链路的第 3 环。

注意：每次调用都会产生 API 费用。默认走 `seedance-2.0-guanfang`（SD 2.0 官方全功能版）。

用法：
  D:/DevTools/Python312/python.exe scripts/poc_生成绿幕视频.py \
      --first-frame "output/宠物动作-建国-v1-2026-09-03/01-首帧/绿幕首帧-1024.png" \
      --prompt-file "output/宠物动作-建国-v1-2026-09-03/02-提示词/Seedance提示词-01-组合循环.txt" \
      --outdir "output/宠物动作-建国-v1-2026-09-03/03-视频" \
      --duration 12

产物：
  03-视频/绿幕视频-<taskId>.mp4
  03-视频/生成记录.json

密钥与模型从 services/appearance-generation/.env 读取（不写入版本库）。
走的是产品同一份客户端代码 `photo_avatar_backend.lk888_client`，不做第二份实现。
"""
from __future__ import annotations

import argparse
import json
import re
import sys
import time
from dataclasses import replace
from pathlib import Path

import httpx

ROOT = Path(__file__).resolve().parents[1]
SERVICE_SRC = ROOT / "services" / "appearance-generation" / "src"
sys.path.insert(0, str(SERVICE_SRC))

from photo_avatar_backend.config import BackendConfig, ConfigError  # noqa: E402
from photo_avatar_backend.lk888_client import Lk888Client, Lk888Error  # noqa: E402

PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
ENV_FILE = ROOT / "services" / "appearance-generation" / ".env"
POLL_INTERVAL = 10.0


def load_env(path: Path) -> dict[str, str]:
    env: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        env[key.strip()] = value.strip()
    return env


_SECTION_NUM = re.compile(r"^[一二三四五六七八九十]+、")
_TOP_SECTION_PREFIXES = ("主提示词", "负向提示词", "背景", "为什么")


def _is_section_header(line: str) -> bool:
    """顶层分节标题：以【 + 中文数字编号 + 、 开头，或以主提示词/负向提示词/背景/为什么开头。

    正文里的子标题（【动作安排…】【眼睛…】【首尾一致性…】【严格要求】等）不带编号、
    也不是这些前缀，返回 False，不会被误判为分节边界。
    """
    stripped = line.strip()
    if not (stripped.startswith("【") and stripped.endswith("】")):
        return False
    inner = stripped[1:-1]
    return bool(_SECTION_NUM.match(inner)) or inner.startswith(_TOP_SECTION_PREFIXES)


def section(lines: list[str], header_keyword: str) -> str:
    """取 【xxx】 分节标题下、到下一个顶层分节标题或分隔行为止的正文（含内部子标题）。"""
    start = None
    for index, line in enumerate(lines):
        if header_keyword in line and _is_section_header(line):
            start = index + 1
            break
    if start is None:
        return ""

    body: list[str] = []
    for line in lines[start:]:
        stripped = line.strip()
        if stripped and set(stripped) == {"-"}:  # 分隔行：出现在正文之后即结束
            if body:
                break
            continue
        if _is_section_header(line):
            break
        body.append(line)
    return "\n".join(body).strip()


def build_prompt(prompt_file: Path, include_negative: bool) -> tuple[str, bool]:
    text = prompt_file.read_text(encoding="utf-8")
    lines = text.splitlines()

    main = section(lines, "主提示词")
    if not main:
        # 没有约定标题的老提示词：整份直接用
        return text.strip(), False

    negative = section(lines, "负向提示词") if include_negative else ""
    if negative:
        main = f"{main}\n\nAvoid the following: {negative}"
    return main, bool(negative)


def main() -> int:
    parser = argparse.ArgumentParser(description="提交绿幕首帧，生成 Seedance 动作视频")
    parser.add_argument("--first-frame", required=True, help="绿幕首帧 PNG（图生视频首帧）")
    parser.add_argument("--prompt-file", required=True, help="Seedance 提示词 txt")
    parser.add_argument("--outdir", required=True, help="输出目录")
    parser.add_argument("--version", default="标准", choices=["Mini", "快速", "标准"],
                        help="速度版本；Mini/快速 只能 480p/720p")
    parser.add_argument("--duration", default="5",
                        help="时长秒数：auto 或 4-15")
    parser.add_argument("--resolution", default="720p",
                        choices=["480p", "720p", "1080p", "4K"],
                        help="分辨率；1080p/4K 需要 --version 标准")
    parser.add_argument("--aspect-ratio", default="1:1", help="画幅比例，默认 1:1")
    parser.add_argument("--mode", default="shouweizhen",
                        choices=["shouweizhen", "cankaosheng"],
                        help="shouweizhen=首尾帧（1 张=首帧），cankaosheng=参考生")
    parser.add_argument("--model", default=None,
                        help="覆盖视频模型名（默认用 config.video_model）；"
                             "按秒版用 seedance-2.0-guanfang-anmiao 可预估总价")
    parser.add_argument("--no-negative", dest="include_negative",
                        action="store_false", default=True,
                        help="不把【负向提示词】追加到主提示词后面")
    parser.add_argument("--timeout", type=float, default=900.0, help="轮询超时秒数")
    parser.add_argument("--yes", action="store_true", help="跳过费用确认")
    args = parser.parse_args()

    frame = Path(args.first_frame).resolve()
    if not frame.is_file():
        raise SystemExit(f"[缺输入] 首帧不存在: {frame}")
    if not frame.read_bytes().startswith(PNG_SIGNATURE):
        raise SystemExit(
            f"[缺输入] 首帧必须是 PNG（平台按 data:image/png 内联上传）: {frame}\n"
            "        poc_绿幕首帧.py 同目录会同时产出 .png，用那个。"
        )
    prompt_file = Path(args.prompt_file).resolve()
    if not prompt_file.is_file():
        raise SystemExit(f"[缺输入] 提示词不存在: {prompt_file}")
    if not ENV_FILE.is_file():
        raise SystemExit(f"[缺配置] 找不到 {ENV_FILE}")

    env = load_env(ENV_FILE)
    # 固定状态目录：config 会按 cwd 解析它并做「仓库内必须落在 output/ 下」的校验，
    # 换个工作目录跑就会误报。本脚本不读不写状态目录，这里只为让 from_env 走通
    # （顺便保留它的模型白名单校验，模型名不会因为环境变量写错而放飞）。
    env["PHOTO_AVATAR_BACKEND_STATE_DIR"] = str(ROOT / "output" / "photo-avatar-backend")
    try:
        config = BackendConfig.from_env(env)
    except ConfigError as exc:
        raise SystemExit(f"[缺配置] {exc}") from exc
    if args.model:
        config = replace(config, video_model=args.model)

    prompt, negative_used = build_prompt(prompt_file, args.include_negative)
    out_dir = Path(args.outdir).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    with httpx.Client(follow_redirects=False) as http:
        client = Lk888Client(config, http)
        params_note = (
            f"模型={config.video_model}  版本={args.version}  时长={args.duration}s  "
            f"分辨率={args.resolution}  画幅={args.aspect_ratio}  模式={args.mode}"
        )
        print(f"[提交] {params_note}")
        print(f"       首帧: {frame}")
        print(f"       提示词: {prompt_file}（{len(prompt)} 字，负向词{'已追加' if negative_used else '未追加'}）")

        if not args.yes:
            answer = input("       会产生 API 费用，继续请输入 y: ").strip().lower()
            if answer != "y":
                print("[取消] 未提交")
                return 1

        try:
            task_id = client.submit_video(
                prompt,
                images=[frame.read_bytes()],
                version=args.version,
                duration=args.duration,
                resolution=args.resolution,
                aspect_ratio=args.aspect_ratio,
                mode=args.mode,
            )
        except Lk888Error as exc:
            raise SystemExit(f"[提交失败] {exc.code}: {exc} {exc.diagnostic}") from exc

        print(f"[任务] task_id = {task_id}，开始轮询（最多 {int(args.timeout)}s）")
        state = client.poll_image(task_id)
        last_state = None
        waited = 0.0
        while not state.is_final:
            if state.state != last_state:
                print(f"       状态: {state.state}  ({int(waited)}s)")
                last_state = state.state
            if waited >= args.timeout:
                raise SystemExit(
                    f"[超时] {int(waited)}s 内未完成，task_id={task_id} 可另行查询"
                )
            time.sleep(POLL_INTERVAL)
            waited += POLL_INTERVAL
            state = client.poll_image(task_id)

        if state.state != "success" or not state.result_url:
            detail = state.error.code if state.error else state.state
            raise SystemExit(f"[生成失败] state={state.state} {detail}")

        print(f"[下载] {state.result_url}")
        try:
            video = client.download_video(state.result_url)
        except Lk888Error as exc:
            raise SystemExit(f"[下载失败] {exc.code}: {exc}") from exc

    target = out_dir / f"绿幕视频-{task_id}.mp4"
    target.write_bytes(video)
    print(f"[保存] {target}  ({len(video) // 1024} KB)")

    (out_dir / "生成记录.json").write_text(
        json.dumps(
            {
                "model": config.video_model,
                "params": {
                    "version": args.version,
                    "duration": args.duration,
                    "resolution": args.resolution,
                    "aspectRatio": args.aspect_ratio,
                    "mode": args.mode,
                },
                "jobs": [
                    {
                        "taskId": task_id,
                        "state": "success",
                        "outputPath": str(target.relative_to(ROOT)),
                        "bytes": len(video),
                        "resultUrl": state.result_url,
                        "prompt": prompt,
                        "negativePromptAppended": negative_used,
                        "sourceFrame": str(frame),
                        "sourcePromptFile": str(prompt_file),
                    }
                ],
            },
            ensure_ascii=False,
            indent=2,
        ),
        encoding="utf-8",
    )
    print("\n[下一步] 抠像（复用 idle 取景框，禁止 autocrop）：")
    print(
        f'    D:/DevTools/Python312/python.exe scripts/poc_抠像.py '
        f'--video "{target}" --no-autocrop --size 0 '
        f'--out "{out_dir.parent / "04-抠像"}"'
    )
    print("[注意] 抠像前先按 skill `petbaby-greenscreen-matting` 的四项判据看图，"
          "并确认视频里没有出画/多手/构图跳变。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
