"""逐帧重绘 demo v6：基于真实猫呼吸生理 + 实际看图后的定位。

看图后修正（session-4d34 像素猫 1024×1024 → 512 缩放）：
- 头身分界 y≈260（胸毛区开始）
- 前爪并排起点 y≈390
- 前爪底（地面接触点）y≈500
- 尾巴在右下角 y≈400-505

真实猫呼吸生理（参考兽医资料）：
- 静息呼吸 20-30 次/分（一个完整周期 2-3 秒）
- 吸气主动、较快；呼气被动、较慢（吸呼比约 1:1.5）
- 可见运动：肋骨/胸廓起伏，腹部微微外扩
- 头会随胸廓轻微上下浮动（坐姿时可见）
- 四肢/爪子/尾巴静止（地面接触点和稳定锚点）

v5 错在哪：
1. 头锁死 → 真实呼吸头会随胸廓轻微浮动
2. 整块身体（含前爪）一起平移 → 看起来猫在蹦
3. y=300 整齐平移 → 腰部硬接缝
4. 吸呼对称 → 真实不对称的
5. 没考虑锚点

v6 设计：
- 锚点：前爪（y>=440）+ 尾巴 + 腹底
- 运动区：头 + 胸 + 上腹（y=0 到 y=440）整体随呼吸微起伏
- 幅度 1-2px（512 高时 2px ≈ 显示 256 时 1px，符合真实比例）
- 节奏：16 帧 / 2.88s ≈ 21 次/分
  - 吸气 5 帧（0.9s）：0, 1, 1, 2, 2
  - 峰停 1 帧：2
  - 呼气 8 帧（1.4s）：2, 1, 1, 1, 0, 0, 0, 0
  - 谷停 2 帧：0, 0
- 边界处理：在 y=440-位移 行复制上一行像素填补，无 alpha 缝
"""
import os
import sys

import numpy as np
from PIL import Image

LOGICAL = 512

# 基于看图后的实测定位
Y_ANCHOR = 440       # 呼吸运动的下边界（在前爪起点之上）
Y_HEAD_BODY = 260    # 头/身分界（仅供分析参考，不用于切割）
Y_PAW_BOTTOM = 500   # 前爪底（地面接触点，绝不能动）

EYES = {
    "left":  {"box": (178, 150, 208, 188), "cy": 169},
    "right": {"box": (238, 150, 282, 188), "cy": 169},
}


def load(path):
    img = Image.open(path).convert("RGBA")
    if img.width != LOGICAL:
        img = img.resize((LOGICAL, LOGICAL), Image.BOX)
    return np.asarray(img).astype(np.int16)


def eye_surround(a, box):
    """取眼周毛色，用于眨眼时填充。"""
    x0, y0, x1, y1 = box
    x0, y0 = max(0, x0), max(0, y0)
    x1, y1 = min(LOGICAL, x1), min(LOGICAL, y1)
    ring = np.concatenate([
        a[y0, x0:x1, :3], a[y1 - 1, x0:x1, :3],
        a[y0:y1, x0, :3], a[y0:y1, x1 - 1, :3],
    ])
    return ring.mean(axis=0).astype(np.uint8)


def breath_frame_v6(a, rise):
    """
    真实呼吸运动：头+胸+上腹（y < Y_ANCHOR）整体向上微浮 `rise` 像素。
    锚点（y >= Y_ANCHOR：前爪 + 下腹 + 尾巴）完全不动。
    边界处理：被移动的最后一行的下方用复制其本身填充，避免接缝。
    """
    if rise == 0:
        return a.copy()
    out = a.copy()
    # 上半身向上平移 rise 像素
    # out[0:Y_ANCHOR-rise] = a[rise:Y_ANCHOR]
    # out[Y_ANCHOR-rise:Y_ANCHOR] = a[Y_ANCHOR-1]（复制原锚点行）→ 无缝
    if rise > 0:
        out[:Y_ANCHOR - rise] = a[rise:Y_ANCHOR]
        # 填补被腾出的底部行：用原图 Y_ANCHOR-1 行复制
        anchor_row = a[Y_ANCHOR - 1:Y_ANCHOR, :].repeat(rise, axis=0)
        out[Y_ANCHOR - rise:Y_ANCHOR] = anchor_row
    return out


# 16 帧呼吸位移序列（单位：像素，正值=上浮）
# 周期 2.88s ≈ 21 次/分
# 吸气 5 帧 (0.9s) → 峰停 1 帧 → 呼气 8 帧 (1.4s) → 谷停 2 帧
BREATH_SEQ = [
    0,  # 0  谷
    1,  # 1  吸-
    1,  # 2  吸
    2,  # 3  吸
    2,  # 4  吸
    2,  # 5  峰停
    1,  # 6  呼
    1,  # 7  呼
    1,  # 8  呼
    0,  # 9  呼
    0,  # 10 呼
    0,  # 11 呼
    0,  # 12 呼
    0,  # 13 谷
    0,  # 14 谷
    0,  # 15 谷
]
FRAME_MS = 180  # 单帧时长


def blink_frame(a, surround, level):
    """眨眼：覆盖眼睛像素 + 画眼睑线。level 0=全开, 0.5=半闭, 1.0=全闭。"""
    out = a.copy()
    for eye in EYES.values():
        x0, y0, x1, y1 = eye["box"]
        if level >= 1.0:
            continue
        elif level >= 0.5:
            h = y1 - y0
            cut = max(1, int(h * 0.30))
            out[y0:y0 + cut, x0:x1, :3] = surround
            out[y1 - cut:y1, x0:x1, :3] = surround
        else:
            out[y0:y1, x0:x1, :3] = surround
            cy = eye["cy"]
            lid = np.array([96, 62, 58], dtype=np.uint8)
            out[cy - 1, x0:x1, :3] = lid
            out[cy, x0:x1, :3] = lid
    return out


def main(path):
    a = load(path)
    out_dir = "output/pixel-motion-demo/v6"
    os.makedirs(out_dir, exist_ok=True)

    surround = eye_surround(a, EYES["left"]["box"])
    print(f"眼周毛色: {surround.tolist()}")
    print(f"位移序列: {BREATH_SEQ}")
    print(f"周期: {len(BREATH_SEQ) * FRAME_MS}ms = {len(BREATH_SEQ) * FRAME_MS / 1000:.2f}s ≈ {60 / (len(BREATH_SEQ) * FRAME_MS / 1000):.0f} 次/分")

    # 呼吸 GIF（16 帧独立完整帧，不合并相同帧）
    breath_frames = [Image.fromarray(breath_frame_v6(a, d).astype(np.uint8)) for d in BREATH_SEQ]
    breath_frames[0].save(
        f"{out_dir}/breath.gif",
        save_all=True, append_images=breath_frames[1:],
        duration=FRAME_MS, loop=0, disposal=2, optimize=False,
    )

    # 眨眼 GIF（5 帧）
    blink_levels = [1.0, 0.5, 0.0, 0.5, 1.0]
    blink_frames = [Image.fromarray(blink_frame(a, surround, lv).astype(np.uint8)) for lv in blink_levels]
    blink_frames[0].save(
        f"{out_dir}/blink.gif",
        save_all=True, append_images=blink_frames[1:],
        duration=150, loop=0, disposal=2, optimize=False,
    )

    # 关键帧大图（rest / 峰吸 / 闭眼）
    Image.fromarray(a.astype(np.uint8)).resize((1024, 1024), Image.NEAREST).save(f"{out_dir}/rest.png")
    Image.fromarray(breath_frame_v6(a, 2).astype(np.uint8)).resize((1024, 1024), Image.NEAREST).save(f"{out_dir}/peak_inhale.png")
    Image.fromarray(blink_frame(a, surround, 0.0).astype(np.uint8)).resize((1024, 1024), Image.NEAREST).save(f"{out_dir}/closed.png")

    print(f"已输出 {out_dir}/breath.gif, blink.gif, rest.png, peak_inhale.png, closed.png")


if __name__ == "__main__":
    main(sys.argv[1])
