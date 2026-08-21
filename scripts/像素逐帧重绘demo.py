"""逐帧重绘 demo v2：呼吸 + 眨眼。眼睛/头身分界用实测定位硬编码。"""
import sys
import os

import numpy as np
from PIL import Image

LOGICAL = 512
Y_SPLIT = 300  # 头身分界（实测）
EYES = {
    "left": {"box": (178, 150, 208, 188), "cy": 169},
    "right": {"box": (238, 150, 282, 188), "cy": 169},
}


def load(path):
    img = Image.open(path).convert("RGBA")
    if img.width != LOGICAL:
        img = img.resize((LOGICAL, LOGICAL), Image.BOX)
    return np.asarray(img).astype(np.int16)


def eye_surround(a, box):
    x0, y0, x1, y1 = box
    x0, y0 = max(0, x0), max(0, y0)
    x1, y1 = min(LOGICAL, x1), min(LOGICAL, y1)
    ring = np.concatenate([
        a[y0, x0:x1, :3], a[y1 - 1, x0:x1, :3],
        a[y0:y1, x0, :3], a[y0:y1, x1 - 1, :3],
    ])
    return ring.mean(axis=0).astype(np.uint8)


def breath_frame(a, dy):
    """身体(y>=Y_SPLIT)整行位移 dy（-1吸气上移/+1呼气下移），头不动，alpha一起动。"""
    if dy == 0:
        return a.copy()
    out = a.copy()
    mask = a[:, :, 3] > 128
    ys = np.where(mask)[0]
    bottom = ys.max()
    if dy < 0:  # 上移
        out[Y_SPLIT:bottom, :] = a[Y_SPLIT + 1:bottom + 1, :]
        out[bottom, :] = a[bottom, :]
    else:  # 下移
        out[Y_SPLIT + 1:bottom + 1, :] = a[Y_SPLIT:bottom, :]
        out[Y_SPLIT, :] = a[Y_SPLIT, :]
    return out


def blink_frame(a, surround, level):
    out = a.copy()
    for eye in EYES.values():
        x0, y0, x1, y1 = eye["box"]
        if level >= 1.0:
            continue
        elif level >= 0.5:
            h = y1 - y0
            cut = max(1, int(h * 0.22))
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
    os.makedirs("output/pixel-motion-demo/v5", exist_ok=True)
    base = "output/pixel-motion-demo/v5"

    surround = eye_surround(a, EYES["left"]["box"])
    print(f"眼周毛色: {surround.tolist()}")

    # 呼吸 8 帧（缓慢起伏，吸气-停-呼气-停）
    dys = [0, -1, 0, 0, 1, 0, 0, -1]
    bf = [Image.fromarray(breath_frame(a, d).astype(np.uint8)) for d in dys]
    bf[0].save(f"{base}/breath.gif", save_all=True, append_images=bf[1:], duration=180, loop=0)

    # 眨眼 5 帧
    levels = [1.0, 0.5, 0.0, 0.5, 1.0]
    kf = [Image.fromarray(blink_frame(a, surround, lv).astype(np.uint8)) for lv in levels]
    kf[0].save(f"{base}/blink.gif", save_all=True, append_images=kf[1:], duration=150, loop=0)

    # 静止/吸气/闭眼 单帧放大图
    Image.fromarray(a.astype(np.uint8)).resize((1024, 1024), Image.NEAREST).save(f"{base}/rest.png")
    Image.fromarray(breath_frame(a, -1).astype(np.uint8)).resize((1024, 1024), Image.NEAREST).save(f"{base}/inhale.png")
    Image.fromarray(blink_frame(a, surround, 0.0).astype(np.uint8)).resize((1024, 1024), Image.NEAREST).save(f"{base}/closed.png")

    print(f"已输出 {base}/breath.gif, blink.gif, rest.png, inhale.png, closed.png")


if __name__ == "__main__":
    main(sys.argv[1])
