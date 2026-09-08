# -*- coding: utf-8 -*-
"""POC：用 lk888 + gpt-image-2 从宠物照片生成透明母版。

注意：每次调用都会产生 API 费用。默认一次只生成 1 张。

用法：
  D:/DevTools/Python312/python.exe scripts/poc_生成母版.py \
      --photo "C:/Users/Administrator/Desktop/wangjianguo/毛砌墙.jpg" \
      --prompt-file output/宠物动作-毛砌墙-2026-08-30/02-提示词/母版生成提示词.txt \
      --outdir  output/宠物动作-毛砌墙-2026-08-30/00-母版

产物：
  00-母版/母版-<taskId>.png
  00-母版/生成记录.json

密钥从 services/appearance-generation/.env 读取（不写入版本库）。
"""
from __future__ import annotations

import argparse
import base64
import json
import mimetypes
import sys
import time
from pathlib import Path

import httpx

ROOT = Path(__file__).resolve().parents[1]
ENV_FILE = ROOT / "services" / "appearance-generation" / ".env"

BASE_URL = "https://api.lk888.ai"
IMAGE_MODEL = "gpt-image-2"
POLL_INTERVAL = 8.0
POLL_TIMEOUT = 900.0


def load_env(path: Path) -> dict[str, str]:
    env: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        env[key.strip()] = value.strip()
    return env


def data_url(path: Path) -> str:
    mime = mimetypes.guess_type(path.name)[0] or "image/jpeg"
    raw = path.read_bytes()
    return f"data:{mime};base64," + base64.b64encode(raw).decode("ascii")


def submit(client: httpx.Client, api_key: str, prompt: str, photo: Path) -> str:
    resp = client.post(
        f"{BASE_URL}/v1/media/generate",
        headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"},
        json={
            "model": IMAGE_MODEL,
            "prompt": prompt,
            # 平台固定输出 2048x2048，后面流程里会显式缩到 1024
            "params": {"size": "2048x2048", "quality": "auto", "images": [data_url(photo)]},
        },
        timeout=300,
    )
    wire = resp.json()
    if not isinstance(wire, dict) or wire.get("code") != 200:
        raise SystemExit(f"[提交失败] HTTP {resp.status_code}\n{json.dumps(wire, ensure_ascii=False)[:800]}")
    data = wire.get("data") or {}
    task_id = data.get("task_id")
    if not task_id:
        raise SystemExit(f"[提交失败] 响应缺少 task_id\n{json.dumps(wire, ensure_ascii=False)[:800]}")
    return str(task_id)


def poll(client: httpx.Client, api_key: str, task_id: str) -> dict:
    deadline = time.time() + POLL_TIMEOUT
    last = None
    while time.time() < deadline:
        resp = client.get(
            f"{BASE_URL}/v1/skills/task-status",
            headers={"Authorization": f"Bearer {api_key}"},
            params={"task_id": task_id},
            timeout=30,
        )
        wire = resp.json()
        state = wire.get("state")
        if state != last:
            print(f"  状态: {state}")
            last = state
        if state == "success":
            return wire
        if state in {"failed", "cancelled"}:
            raise SystemExit(f"[生成失败] state={state} error={wire.get('error')}")
        time.sleep(POLL_INTERVAL)
    raise SystemExit(f"[超时] {POLL_TIMEOUT}s 内未完成任务 {task_id}")


def main() -> int:
    parser = argparse.ArgumentParser(description="从宠物照片生成透明母版")
    parser.add_argument("--photo", required=True, help="宠物照片路径")
    parser.add_argument("--prompt-file", required=True, help="提示词文件路径")
    parser.add_argument("--outdir", required=True, help="输出目录")
    parser.add_argument("--count", type=int, default=1, help="生成张数（会产生费用）")
    parser.add_argument("--yes", action="store_true", help="跳过费用确认")
    args = parser.parse_args()

    photo = Path(args.photo)
    if not photo.is_file():
        raise SystemExit(f"[缺输入] 照片不存在: {photo}")
    prompt_file = Path(args.prompt_file)
    if not prompt_file.is_file():
        raise SystemExit(f"[缺输入] 提示词文件不存在: {prompt_file}")
    if not ENV_FILE.is_file():
        raise SystemExit(f"[缺配置] 找不到 {ENV_FILE}")

    env = load_env(ENV_FILE)
    api_key = env.get("LK888_API_KEY")
    if not api_key:
        raise SystemExit("[缺配置] .env 里没有 LK888_API_KEY")

    # resolve 一次，后面 relative_to(ROOT) 才能正常工作
    out_dir = Path(args.outdir).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    # 提示词文件里带说明段落，只取【母版生成提示词】分隔线之间的内容
    text = prompt_file.read_text(encoding="utf-8")
    prompt = text
    if "-----" in text:
        parts = text.split("-----")
        # 取最长的一段英文正文
        candidates = [p.strip() for p in parts if "Use the uploaded photo" in p]
        if candidates:
            prompt = max(candidates, key=len).lstrip("-").strip()

    if not args.yes:
        print(f"[确认] 即将调用 lk888 {IMAGE_MODEL} 生成 {args.count} 张母版（会产生费用）")
        print(f"      照片: {photo}")
        print(f"      输出: {out_dir}")
        answer = input("      继续请输入 y: ").strip().lower()
        if answer != "y":
            print("[取消] 未提交")
            return 1

    records = []
    with httpx.Client() as client:
        for i in range(args.count):
            print(f"[提交] 第 {i + 1}/{args.count} 张 ...")
            task_id = submit(client, api_key, prompt, photo)
            print(f"      task_id = {task_id}")
            result = poll(client, api_key, task_id)
            url = result["result_url"]
            print(f"[下载] {url}")
            content = client.get(url, timeout=300).content
            target = out_dir / f"母版-{task_id}.png"
            target.write_bytes(content)
            print(f"[保存] {target}  ({len(content) // 1024} KB)")
            records.append({
                "taskId": task_id,
                "state": "success",
                "outputPath": str(target.relative_to(ROOT)),
                "bytes": len(content),
                "prompt": prompt,
                "sourcePhoto": str(photo),
                "sourcePhotoSha256": None,
            })

    (out_dir / "生成记录.json").write_text(
        json.dumps({
            "photo": str(photo),
            "model": IMAGE_MODEL,
            "count": len(records),
            "jobs": records,
        }, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    print(f"\n[完成] {len(records)} 张 -> {out_dir}")
    print("[下一步] 生成绿幕首帧：")
    print(f"    D:/DevTools/Python312/python.exe scripts/poc_绿幕首帧.py "
          f"--master \"{out_dir / ('母版-' + records[0]['taskId'] + '.png')}\" "
          f"--scale 0.9 --margin-left 0.05 --outdir {out_dir.parent / '01-首帧'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
