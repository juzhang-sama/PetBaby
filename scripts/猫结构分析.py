"""精确分析成品猫结构：头身分界、眼睛、身体边界（512 逻辑网格）。"""
import sys
import numpy as np
from PIL import Image

LOGICAL = 512


def load(path):
    img = Image.open(path).convert("RGBA")
    if img.width != LOGICAL:
        img = img.resize((LOGICAL, LOGICAL), Image.BOX)
    return np.asarray(img).astype(np.int16)


def main(path):
    a = load(path)
    alpha = a[:, :, 3]
    mask = alpha > 128
    ys, xs = np.where(mask)
    print(f"主体: L={xs.min()} T={ys.min()} R={xs.max()} B={ys.max()}")

    # 每行主体宽度轮廓
    widths = []
    for y in range(LOGICAL):
        row_x = np.where(mask[y])[0]
        if len(row_x):
            widths.append((y, row_x.min(), row_x.max(), row_x.max() - row_x.min()))
    print("\n=== 宽度轮廓（每隔 20 行采样）===")
    for y, x0, x1, w in widths[::20]:
        bar = "#" * (w // 8)
        print(f"  y={y:3d}  x[{x0:3d}-{x1:3d}] 宽{w:3d} {bar}")

    # 头身分界：找上半部宽度峰值后，向下找最窄处（脖子/下巴收窄）
    top = ys.min()
    # 头部区域：top 到 top + 0.55*height
    head_bottom_est = top + int((ys.max() - top) * 0.55)
    head_zone = [w for w in widths if w[0] <= head_bottom_est]
    if not head_zone:
        head_zone = widths
    head_peak = max(head_zone, key=lambda w: w[3])
    print(f"\n头部宽度峰值: y={head_peak[0]} 宽{head_peak[3]}")
    # 从峰值向下，找宽度 < 峰值 70% 的第一行 = 脖子
    neck = None
    for y, x0, x1, w in widths:
        if y > head_peak[0] and w < head_peak[3] * 0.72:
            neck = (y, w)
            break
    if neck:
        print(f"脖子收窄: y={neck[0]} 宽{neck[1]}（<峰值72%）")
    else:
        print("未找到明显脖子，用头部55%分界")

    # 眼睛定位：纯黑像素聚类
    rgb_sum = a[:, :, :3].sum(axis=2)
    black = (rgb_sum < 90) & mask
    bys, bxs = np.where(black)
    print(f"\n纯黑像素: {len(bxs)} 个")
    if len(bxs) > 20:
        left = bxs < 256
        for name, px, py in [("左眼", bxs[left], bys[left]), ("右眼", bxs[~left], bys[~left])]:
            if len(px) > 5:
                print(f"  {name}: 质心({px.mean():.0f},{py.mean():.0f}) 范围x[{px.min()}-{px.max()}] y[{py.min()}-{py.max()}]")

    # 身体底部 & 尾巴位置
    print(f"\n主体底部 y={ys.max()} 左侧 x={xs.min()} 右侧 x={xs.max()}")


if __name__ == "__main__":
    main(sys.argv[1])
