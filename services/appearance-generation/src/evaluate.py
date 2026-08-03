# -*- coding: utf-8 -*-
"""Evaluation table rendering for owner review."""
import base64
import io
import json
from pathlib import Path

from PIL import Image


def _to_data_url(image: Image.Image) -> str:
    buffer = io.BytesIO()
    image.save(buffer, format="PNG")
    return "data:image/png;base64," + base64.b64encode(buffer.getvalue()).decode()


def render_evaluation_html(
    sample_name: str,
    photo: Image.Image,
    candidates: list,
    out_path: Path,
) -> None:
    """candidates: list of dicts {index, task_id, image(RGBA), error}."""
    rows = []
    for candidate in candidates:
        if candidate.get("image") is not None and candidate.get("error") is None:
            rows.append(
                f"""
                <div class="candidate">
                  <img src="{_to_data_url(candidate['image'])}" alt="candidate" />
                  <div class="meta">候选 {candidate['index'] + 1} · task {candidate.get('task_id', '')}</div>
                  <textarea placeholder="主人备注 / 是否接受"></textarea>
                </div>
                """
            )
        else:
            rows.append(
                f"""
                <div class="candidate failed">
                  <div class="meta">候选 {candidate['index'] + 1} 失败：{candidate.get('error', 'unknown')}</div>
                </div>
                """
            )
    html = f"""<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8"/>
<title>评估：{sample_name}</title>
<style>
 body {{ font-family: system-ui, sans-serif; margin: 24px; background:#f5f6f8; }}
 .row {{ display:flex; gap:16px; flex-wrap:wrap; }}
 .candidate {{ width: 320px; background:#fff; border:1px solid #ddd; border-radius:8px; padding:10px; }}
 .candidate img {{ width:100%; border-radius:6px; background: linear-gradient(45deg,#eee 25%,#fff 25%,#fff 50%,#eee 50%,#eee 75%,#fff 75%); background-size:16px 16px; }}
 .candidate.failed {{ color:#a33; }}
 .meta {{ font-size:12px; color:#666; margin:6px 0; }}
 textarea {{ width:100%; height:56px; }}
 .photo img {{ width:320px; border-radius:8px; }}
 h1 {{ font-size:18px; }}
</style></head><body>
<h1>{sample_name}</h1>
<div class="row"><div class="photo"><img src="{_to_data_url(photo.convert("RGB"))}" alt="reference"/><div class="meta">参考照片</div></div></div>
<h2>候选（已去背景）</h2>
<div class="row">{''.join(rows)}</div>
</body></html>"""
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(html, encoding="utf-8")


def save_evaluation_json(
    sample_name: str,
    candidates: list,
    out_path: Path,
) -> None:
    data = {
        "sample": sample_name,
        "candidates": [
            {
                "index": c.get("index"),
                "task_id": c.get("task_id"),
                "error": c.get("error"),
                "accepted": None,  # filled by the owner
                "notes": "",
            }
            for c in candidates
        ],
    }
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")
