"""皮肤-骨架分离验证：程序绘制标准模板掩码 + 皮肤数据填色合成。

模板 = 几何掩码（1.8 头身大头坐姿，所有宠物共享，一次性打磨）
皮肤 = 从成品猫提取的数据（主色/花纹 patch/眼色/描边色）
合成 = 逐像素填色，无拼接（部件只是填色区域，不是独立图片）

输出：
- cat-template.png       512 逻辑网格合成猫
- cat-template-2048.png  放大到 2048（与成品图同规格）
- compare.png            原图(512) vs 合成(512) 并排对比
- breath.gif / blink.gif 动作帧演示（模板掩码驱动）
"""
import json
import os

import numpy as np
from PIL import Image

LOGICAL = 512
SEED = 42
rng = np.random.default_rng(SEED)

SKIN = json.load(open("output/pixel-motion-demo/v3/skin-data.json", encoding="utf-8"))
MAIN = np.array(SKIN["mainColor"], dtype=np.uint8)
OUTLINE = np.array(SKIN["outlineColor"], dtype=np.uint8)
EYE = np.array(SKIN["eyeColor"], dtype=np.uint8)
NOSE = np.array([240, 150, 150], dtype=np.uint8)  # 浅粉鼻
PATCH = np.asarray(Image.open("output/pixel-motion-demo/v3/pattern-patch.png").convert("RGBA"))
os.makedirs("output/pixel-motion-demo/v3", exist_ok=True)


def canvas() -> np.ndarray:
    return np.zeros((LOGICAL, LOGICAL, 4), dtype=np.uint8)


def ellipse_mask(cx: float, cy: float, rx: float, ry: float) -> np.ndarray:
    y, x = np.ogrid[:LOGICAL, :LOGICAL]
    return ((x - cx) / rx) ** 2 + ((y - cy) / ry) ** 2 <= 1.0


def triangle_mask(p0, p1, p2) -> np.ndarray:
    y, x = np.ogrid[:LOGICAL, :LOGICAL]
    pts = np.stack([np.broadcast_to(x, (LOGICAL, LOGICAL)), np.broadcast_to(y, (LOGICAL, LOGICAL))], axis=-1).astype(float)
    def cross2(a, b):
        return a[..., 0] * b[..., 1] - a[..., 1] * b[..., 0]
    d1 = cross2(np.array(p1) - np.array(p0), pts - np.array(p0))
    d2 = cross2(np.array(p2) - np.array(p1), pts - np.array(p1))
    d3 = cross2(np.array(p0) - np.array(p2), pts - np.array(p2))
    has_neg = (d1 < 0) | (d2 < 0) | (d3 < 0)
    has_pos = (d1 > 0) | (d2 > 0) | (d3 > 0)
    return ~(has_neg & has_pos)


def dilate(m: np.ndarray, n: int = 1) -> np.ndarray:
    out = m.copy()
    for _ in range(n):
        out = (
            out
            | np.roll(out, 1, 0) | np.roll(out, -1, 0)
            | np.roll(out, 1, 1) | np.roll(out, -1, 1)
        )
    return out


def fill(c: np.ndarray, mask: np.ndarray, color) -> None:
    color = np.array(color, dtype=np.uint8)
    c[mask] = np.concatenate([color, [255]])


def clean_patch(patch: np.ndarray, main, outline, keep_min: int = 100) -> np.ndarray:
    """只保留花纹色像素（剔除描边黑/白底），其余置透明——patch 变成纯花纹笔刷。"""
    out = patch.copy()
    rgb = patch[:, :, :3].astype(int)
    d_main = np.abs(rgb - np.array(main, dtype=int)).sum(axis=2)
    d_out = np.abs(rgb - np.array(outline, dtype=int)).sum(axis=2)
    keep = (d_main > keep_min) & (d_out > keep_min)
    out[:, :, 3] = np.where(keep, patch[:, :, 3], 0)
    return out


def paste_patch(c: np.ndarray, patch: np.ndarray, cx: int, cy: int, scale: float = 1.0) -> None:
    """alpha 混合贴花纹 patch（带真实纹理）。"""
    p = Image.fromarray(patch)
    if scale != 1.0:
        p = p.resize((max(4, int(p.width * scale)), max(4, int(p.height * scale))), Image.NEAREST)
    arr = np.asarray(p)
    ph, pw = arr.shape[:2]
    x0, y0 = cx - pw // 2, cy - ph // 2
    x1, y1 = x0 + pw, y0 + ph
    # 裁剪到画布
    sx0, sy0 = max(0, -x0), max(0, -y0)
    dx0, dy0 = max(0, x0), max(0, y0)
    dx1, dy1 = min(LOGICAL, x1), min(LOGICAL, y1)
    if dx1 <= dx0 or dy1 <= dy0:
        return
    src = arr[sy0: sy0 + (dy1 - dy0), sx0: sx0 + (dx1 - dx0)]
    dst = c[dy0:dy1, dx0:dx1]
    alpha = src[:, :, 3:4].astype(np.float32) / 255.0
    out = (dst.astype(np.float32) * (1 - alpha) + src.astype(np.float32) * alpha).astype(np.uint8)
    c[dy0:dy1, dx0:dx1] = out


def build_template(breath_dy: int = 0, blink: float = 1.0) -> np.ndarray:
    """构建标准模板合成猫。breath_dy: 身体垂直压缩量；blink: 1睁 0闭。"""
    c = canvas()

    # ---- 部件掩码（几何模板，1.8 头身坐姿）----
    tail = np.zeros((LOGICAL, LOGICAL), dtype=bool)
    tail |= ellipse_mask(388, 392, 24, 22)
    tail |= ellipse_mask(416, 372, 20, 18)
    tail |= ellipse_mask(436, 348, 15, 14)
    body = ellipse_mask(256, 348 + breath_dy // 2, 114, 98 - breath_dy // 2)
    paws = ellipse_mask(210, 418, 32, 24) | ellipse_mask(302, 418, 32, 24)
    ears = triangle_mask((168, 96), (140, 26), (216, 52)) | triangle_mask((344, 96), (372, 26), (296, 52))
    head = ellipse_mask(256, 158, 120, 104)

    # 图层顺序：tail → body → paws → ears → head（head 盖住耳朵根部）
    fill(c, tail, MAIN)
    fill(c, body, MAIN)
    fill(c, paws, MAIN)
    fill(c, ears, MAIN)
    fill(c, head, MAIN)

    # ---- 花纹：头部多（~30%）、身体少（~8%）、底部无 ----
    # 先清洁 patch：只留棕纹，剔除黑描边与白底
    brush = clean_patch(PATCH, MAIN, OUTLINE)
    head_patch_spots = [(180, 120), (330, 130), (250, 90)]
    for cx, cy in head_patch_spots[:2]:
        paste_patch(c, brush, cx, cy, 0.85)
    paste_patch(c, brush, 300, 380, 0.5)
    paste_patch(c, brush, 210, 300, 0.45)

    # ---- 五官（在头部之上）----
    eye_ry = 15 if blink >= 0.6 else 3
    fill(c, ellipse_mask(208, 148, 13, eye_ry), EYE)
    fill(c, ellipse_mask(304, 148, 13, eye_ry), EYE)
    fill(c, ellipse_mask(256, 196, 7, 6), NOSE)
    # 嘴：鼻子下方两道短线
    c[206:210, 248:253] = np.concatenate([OUTLINE, [255]])
    c[206:210, 259:264] = np.concatenate([OUTLINE, [255]])

    # ---- 外轮廓描边（像素风）----
    subject = c[:, :, 3] > 128
    edge = dilate(subject, 1) & ~subject
    fill(c, edge, OUTLINE)

    return c


def main() -> None:
    cat = build_template()
    Image.fromarray(cat).save("output/pixel-motion-demo/v3/cat-template.png")
    big = Image.fromarray(cat).resize((2048, 2048), Image.NEAREST)
    big.save("output/pixel-motion-demo/v3/cat-template-2048.png")

    # 对比图：原图(512) vs 合成(512)
    src = Image.open(SKIN["source"]).convert("RGBA").resize((512, 512), Image.BOX)
    cmp = Image.new("RGBA", (512 * 2 + 2, 512), (245, 245, 245, 255))
    cmp.paste(src, (0, 0))
    cmp.paste(Image.fromarray(cat), (512 + 2, 0))
    cmp.save("output/pixel-motion-demo/v3/compare.png")

    # 呼吸动画：身体垂直压缩 2px（头顶固定，身体顶部下压），头部不动
    frames = []
    for i in range(6):
        dy = int(round(3 * abs(np.sin(i / 5 * np.pi))))
        frames.append(Image.fromarray(build_template(breath_dy=dy, blink=1.0)))
    frames[0].save("output/pixel-motion-demo/v3/breath.gif", save_all=True, append_images=frames[1:], duration=120, loop=0)

    # 眨眼动画：眼睛 ry 15 → 3 → 15
    frames = []
    for i, blink in enumerate([1.0, 1.0, 0.4, 0.15, 0.4, 1.0]):
        frames.append(Image.fromarray(build_template(blink=blink)))
    frames[0].save("output/pixel-motion-demo/v3/blink.gif", save_all=True, append_images=frames[1:], duration=140, loop=0)

    print("已输出:")
    for f in ["cat-template.png", "cat-template-2048.png", "compare.png", "breath.gif", "blink.gif"]:
        print(f"  output/pixel-motion-demo/v3/{f}")


if __name__ == "__main__":
    main()
