# -*- coding: utf-8 -*-
"""POC：生成独立预览器（不接入产品代码）。

产物是一份自包含 HTML + 一张精灵图，双击即可在浏览器打开：
    - 动画模式：按 manifest 的 frameDurationMs 循环播放，可切黑/白/棋盘格背景
    - 单帧模式：逐帧加载原始分辨率 PNG，用于看胡须、耳缘、尾巴细节

不改动 apps/desktop 任何代码，也不引用产品运行时。

用法：
    D:/DevTools/Python312/python.exe scripts/poc_预览.py
    D:/DevTools/Python312/python.exe scripts/poc_预览.py --cell 320

输出：
    output/POC-绿幕视频-2026-08-29/05-验收/预览.html
    output/POC-绿幕视频-2026-08-29/05-验收/预览-精灵图.png
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
POC_DIR = ROOT / "output" / "POC-绿幕视频-2026-08-29"
FRAMES_DIR = POC_DIR / "04-帧序列" / "frames"
MANIFEST = POC_DIR / "04-帧序列" / "manifest.json"
OUT_DIR = POC_DIR / "05-验收"

MAX_SHEET = 4096

HTML_TEMPLATE = """<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<title>POC 预览 - 绿幕抠像帧序列</title>
<style>
  * {{ box-sizing: border-box; }}
  body {{ margin: 0; padding: 24px; background: #1b1d21; color: #e6e6e6;
         font-family: "Microsoft YaHei", system-ui, sans-serif; }}
  h1 {{ font-size: 17px; margin: 0 0 4px; font-weight: 600; }}
  .sub {{ font-size: 12px; color: #8b929c; margin-bottom: 18px; }}
  .bar {{ display: flex; gap: 10px; align-items: center; flex-wrap: wrap; margin-bottom: 14px; }}
  button {{ background: #2c313a; color: #e6e6e6; border: 1px solid #3d444f;
            padding: 7px 15px; border-radius: 6px; cursor: pointer; font-size: 13px; }}
  button:hover {{ background: #363c47; }}
  button.on {{ background: #3b6fd4; border-color: #4b7fe4; }}
  .stage {{ display: inline-block; border: 1px solid #3d444f; border-radius: 8px;
            overflow: hidden; line-height: 0; background: #000; }}
  canvas {{ display: block; }}
  .meta {{ font-size: 12px; color: #8b929c; margin-top: 12px; line-height: 1.7; }}
  .meta b {{ color: #c9d1d9; font-weight: 600; }}
  input[type=range] {{ width: 260px; vertical-align: middle; }}
  .verdict {{ padding: 3px 9px; border-radius: 4px; font-weight: 600; }}
  .pass {{ background: #1f5c33; color: #8ff0b0; }}
  .fail {{ background: #632020; color: #ffb3b3; }}
</style>
</head>
<body>
<h1>POC 预览：绿幕视频 → 自动抠像 → 透明 PNG 帧序列</h1>
<div class="sub">仅用于验收，不接入产品运行时。样本：扭扭 · 呼吸+摇尾</div>

<div class="bar">
  <button id="btnPlay" class="on">暂停</button>
  <span>背景：</span>
  <button class="bg" data-bg="checker">棋盘格</button>
  <button class="bg on" data-bg="black">黑</button>
  <button class="bg" data-bg="white">白</button>
  <span style="margin-left:12px">模式：</span>
  <button id="mAnim" class="on">动画</button>
  <button id="mSingle">单帧（全分辨率）</button>
</div>

<div class="bar" id="singleBar" style="display:none">
  <span>帧：</span><input type="range" id="frameSlider" min="0" max="{nframes_m1}" value="0">
  <span id="frameLabel">0</span>
</div>

<div class="stage"><canvas id="cv" width="{disp}" height="{disp}"></canvas></div>

<div class="meta">
  <div>帧数 <b>{nframes}</b> · 帧时长 <b>{dur} ms</b>（约 <b>{fps:.1f} fps</b>） ·
       源尺寸 <b>{w}×{h}</b> · 抠像方法 <b>{method}</b></div>
  <div>机械验收：<span class="verdict {vclass}">{verdict}</span>
       <span id="detail" style="color:#8b929c"></span></div>
  <div style="margin-top:8px; color:#6f7783">
    四项标准：尾巴完整 / 胡须耳缘无绿边 / 帧间不闪烁 / 黑白棋盘格自然。<br>
    机械判据只覆盖可量化部分，"自然"最终仍需人眼在黑、白、棋盘格三种背景下确认。
  </div>
</div>

<script>
const CFG = {config};
const cv = document.getElementById('cv');
const ctx = cv.getContext('2d');
let bgMode = 'black', animMode = true, playing = true, idx = 0, timer = null;
let sheet = null, singleImg = null;

function drawChecker(ctx, size, cell) {{
  for (let y = 0; y < size; y += cell)
    for (let x = 0; x < size; x += cell) {{
      ctx.fillStyle = (((x / cell | 0) + (y / cell | 0)) % 2 === 0) ? '#eeeeee' : '#c6c6c6';
      ctx.fillRect(x, y, cell, cell);
    }}
}}

function paintBackground() {{
  const s = cv.width;
  if (bgMode === 'checker') drawChecker(ctx, s, Math.max(8, s / 32 | 0));
  else {{ ctx.fillStyle = bgMode === 'white' ? '#ffffff' : '#000000'; ctx.fillRect(0, 0, s, s); }}
}}

function drawSheetFrame() {{
  paintBackground();
  if (!sheet) return;
  const c = CFG.cell, cols = CFG.cols;
  const sx = (idx % cols) * c, sy = ((idx / cols) | 0) * c;
  ctx.drawImage(sheet, sx, sy, c, c, 0, 0, cv.width, cv.height);
}}

function drawSingle() {{
  paintBackground();
  if (singleImg && singleImg.complete) ctx.drawImage(singleImg, 0, 0, cv.width, cv.height);
}}

function render() {{ animMode ? drawSheetFrame() : drawSingle(); }}

function step() {{
  idx = (idx + 1) % CFG.nframes;
  if (animMode) render(); else loadSingle(idx);
}}

function loadSingle(i) {{
  idx = i;
  document.getElementById('frameSlider').value = i;
  document.getElementById('frameLabel').textContent = i;
  singleImg = new Image();
  singleImg.onload = render;
  singleImg.src = CFG.framesDir + '/' + CFG.frameNames[i];
}}

function startTimer() {{
  stopTimer();
  timer = setInterval(step, CFG.frameDurationMs);
}}
function stopTimer() {{ if (timer) {{ clearInterval(timer); timer = null; }} }}

document.getElementById('btnPlay').onclick = (e) => {{
  playing = !playing;
  e.target.textContent = playing ? '暂停' : '播放';
  e.target.classList.toggle('on', playing);
  playing ? startTimer() : stopTimer();
}};

document.querySelectorAll('.bg').forEach(b => b.onclick = () => {{
  document.querySelectorAll('.bg').forEach(x => x.classList.remove('on'));
  b.classList.add('on');
  bgMode = b.dataset.bg;
  render();
}});

document.getElementById('mAnim').onclick = () => {{
  animMode = true;
  document.getElementById('mAnim').classList.add('on');
  document.getElementById('mSingle').classList.remove('on');
  document.getElementById('singleBar').style.display = 'none';
  render(); playing && startTimer();
}};
document.getElementById('mSingle').onclick = () => {{
  animMode = false;
  document.getElementById('mSingle').classList.add('on');
  document.getElementById('mAnim').classList.remove('on');
  document.getElementById('singleBar').style.display = 'flex';
  document.getElementById('frameSlider').max = CFG.nframes - 1;
  loadSingle(idx); playing && startTimer();
}};
document.getElementById('frameSlider').oninput = (e) => loadSingle(+e.target.value);

sheet = new Image();
sheet.onload = () => {{ render(); startTimer(); }};
sheet.src = CFG.sheetFile;

const rep = CFG.report;
if (rep) {{
  const parts = Object.entries(rep.criteria || {{}})
    .map(([k, v]) => k + ':' + (v.passed ? 'PASS' : 'FAIL'));
  document.getElementById('detail').textContent = '　' + parts.join('　');
}}
</script>
</body>
</html>
"""


def build_sheet(frames: list[Path], cell: int, out_path: Path) -> tuple[int, int]:
    n = len(frames)
    cols = max(1, min(n, MAX_SHEET // cell))
    rows = int(np.ceil(n / cols))
    sheet = Image.new("RGBA", (cols * cell, rows * cell), (0, 0, 0, 0))
    for i, p in enumerate(frames):
        im = Image.open(p).convert("RGBA").resize((cell, cell), Image.LANCZOS)
        sheet.paste(im, ((i % cols) * cell, (i // cols) * cell))
    sheet.save(out_path)
    return cols, rows


def main() -> int:
    parser = argparse.ArgumentParser(description="生成 POC 独立预览器")
    parser.add_argument("--frames", type=str, default=None)
    parser.add_argument("--cell", type=int, default=256, help="精灵图单帧边长")
    parser.add_argument("--disp", type=int, default=512, help="预览画布边长")
    parser.add_argument("--out", type=str, default=None, help="输出目录")
    args = parser.parse_args()

    frames_dir = Path(args.frames) if args.frames else FRAMES_DIR
    if not frames_dir.is_dir():
        raise SystemExit(f"[缺输入] 帧序列不存在: {frames_dir}\n先运行 scripts/poc_抠像.py")
    # 默认输出到"帧序列的上一级"：HTML、精灵图、frames/ 三者同属一棵目录树，
    # 无论用 file:// 直接打开，还是被静态服务器以该目录为根托管，单帧模式都能取到图。
    # 单帧模式用于看胡须/耳缘细节，是验收第 2 项的关键，不能因为路径越界而 404。
    out_dir = Path(args.out) if args.out else frames_dir.parent
    out_dir.mkdir(parents=True, exist_ok=True)

    frames = sorted(frames_dir.glob("f*.png"))
    if not frames:
        raise SystemExit(f"[缺输入] 没有帧文件: {frames_dir}")

    manifest = {}
    if MANIFEST.is_file():
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    action = (manifest.get("actions") or [{}])[0]
    duration = int(action.get("frameDurationMs") or 42)
    names = action.get("frames") or [f"frames/f{i:03d}.png" for i in range(len(frames))]
    names = [Path(n).name for n in names]
    method = "chroma"
    params_path = POC_DIR / "04-帧序列" / "抠像参数.json"
    if params_path.is_file():
        method = json.loads(params_path.read_text(encoding="utf-8")).get("method", "chroma")

    sheet_path = out_dir / "预览-精灵图.png"
    cols, rows = build_sheet(frames, args.cell, sheet_path)
    print(f"[精灵图] {len(frames)} 帧 {args.cell}px  {cols}x{rows}  {sheet_path.stat().st_size // 1024} KB")

    sample = Image.open(frames[0])
    report = None
    for report_path in (out_dir / "验收报告.json", OUT_DIR / "验收报告.json"):
        if report_path.is_file():
            report = json.loads(report_path.read_text(encoding="utf-8"))
            break

    cfg = {
        "nframes": len(frames),
        "frameDurationMs": duration,
        "cell": args.cell,
        "cols": cols,
        "sheetFile": sheet_path.name,
        # 单帧模式按"相对 HTML 所在目录"的路径加载原始分辨率 PNG
        "framesDir": os.path.relpath(frames_dir.resolve(), out_dir.resolve()).replace("\\", "/"),
        "frameNames": names,
        "report": report,
    }
    passed = bool((report or {}).get("overallPassed", False))
    html = HTML_TEMPLATE.format(
        config=json.dumps(cfg, ensure_ascii=False),
        nframes=len(frames), nframes_m1=len(frames) - 1,
        dur=duration, fps=1000.0 / max(1, duration),
        w=sample.size[0], h=sample.size[1], method=method,
        disp=args.disp,
        verdict=("通过" if passed else ("未通过" if report else "尚未验收")),
        vclass=("pass" if passed else "fail"),
    )
    html_path = out_dir / "预览.html"
    html_path.write_text(html, encoding="utf-8")

    print(f"[输出] {html_path}")
    print(f"[输出] {sheet_path}")
    print("\n[使用] 双击 预览.html 在浏览器打开：")
    print("       动画模式看运动是否平滑，单帧模式看胡须/耳缘/尾巴细节，")
    print("       三种背景各看一遍，确认边缘没有绿边和白晕。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
