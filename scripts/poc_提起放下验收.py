# -*- coding: utf-8 -*-
"""poc_提起放下验收.py —— grab-release（提起→放下，一气呵成）视频验收。

设计目标：**通用**。一切从宠物档案 + 抠像参数驱动，绝不 hardcode 某只宠物。
换宠物只需要换 --pet-id，其余（crop box、锚点帧、原始视频、warmup 帧数）自动定位。

用法：
  D:/DevTools/Python312/python.exe scripts/poc_提起放下验收.py --pet-id 04-warm-brown-tabby
  D:/DevTools/Python312/python.exe scripts/poc_提起放下验收.py --pet-id 05-silver-tabby --video <path>

判据（对应 Seedance提示词-grab-release-一气呵成.txt 第五节）：
  1. 末帧 vs 锚点图：bbox 差 < 5px、IoU > 0.95、tailNorm 同号
  2. 头部高度全程不变：全帧 bbox y0 波动 < 15px
  3. 悬空段可循环：循环区间首尾帧 IoU > 0.9
  4. 全帧不超 crop box
  5. 毛色 drift：末帧 vs 锚点平均色差
"""
import os
import json
import argparse
import numpy as np
import cv2

BASE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# 分段比例（对应提示词时间轴 0.8 / 2.3 / 3.8 / 5.0 秒）
SEG_LIFT_END = 0.16
SEG_HOLD_END = 0.46
SEG_DROP_END = 0.76


def imread_any(path):
    return cv2.imdecode(np.fromfile(path, dtype=np.uint8), cv2.IMREAD_UNCHANGED)


def imwrite_any(path, img):
    ok, buf = cv2.imencode(os.path.splitext(path)[1], img)
    if not ok:
        raise IOError("imencode failed: " + path)
    buf.tofile(path)


def green_mask(bgr, thresh=40):
    b = bgr[:, :, 0].astype(np.int16)
    g = bgr[:, :, 1].astype(np.int16)
    r = bgr[:, :, 2].astype(np.int16)
    return (g - np.maximum(r, b)) < thresh


def sample_bg(bgr):
    strip = np.concatenate([
        bgr[:8, :, :].reshape(-1, 3), bgr[-8:, :, :].reshape(-1, 3),
        bgr[:, :8, :].reshape(-1, 3), bgr[:, -8:, :].reshape(-1, 3),
    ])
    return [int(v) for v in np.median(strip, axis=0)[::-1]]


def shape_of(mask):
    ys, xs = np.nonzero(mask)
    if len(xs) == 0:
        return None
    x0, x1 = int(xs.min()), int(xs.max())
    y0, y1 = int(ys.min()), int(ys.max())
    h = y1 - y0 + 1
    w = x1 - x0 + 1
    cx = float(xs.mean())
    low = ys >= (y0 + 0.75 * h)
    cx_low = float(xs[low].mean()) if int(low.sum()) > 50 else cx
    return {
        "bbox": [x0, y0, w, h], "aspect": round(w / h, 3),
        "fillRatio": round(float(mask.sum()) / mask.size, 4),
        "centerX": round(cx, 1), "tailNorm": round((cx_low - cx) / w, 4),
    }


def mask_iou(a, b):
    inter = np.logical_and(a, b).sum()
    union = np.logical_or(a, b).sum()
    return round(float(inter) / float(union), 4) if union else 0.0


def mean_color(bgr, mask):
    px = bgr[mask]
    return [round(float(v), 1) for v in px.mean(axis=0)[::-1]]  # BGR -> RGB


def load_profile(pet_id):
    with open(os.path.join(BASE, "output", "宠物档案", pet_id + ".json"),
              encoding="utf-8") as f:
        return json.load(f)


def locate_anchor(profile):
    """从抠像参数反查 idle 原始视频并按 warmup 抽取锚点帧（通用）。"""
    ref = profile["runtime"]["refParams"].replace("\\", "/")
    with open(os.path.join(BASE, ref), encoding="utf-8") as f:
        params = json.load(f)
    video = params["video"].replace("\\", "/")
    warmup = int(params.get("warmupFramesSkipped", 0))
    cap = cv2.VideoCapture(os.path.join(BASE, video))
    frame = None
    for _ in range(warmup + 1):
        ok, frame = cap.read()
    cap.release()
    if frame is None:
        raise IOError("cannot read anchor frame from " + video)
    return frame, video, warmup


def draw_plot(series_list, colors, vlines, W=900, H=250, legends=()):
    img = np.full((H, W, 3), 255, np.uint8)
    pl, pr, pt, pb = 62, 20, 26, 34
    allv = np.concatenate([np.asarray(s, dtype=float) for s in series_list])
    vmin, vmax = float(allv.min()), float(allv.max())
    rng = max(vmax - vmin, 1.0)
    vmin -= rng * 0.12
    vmax += rng * 0.12
    rng = vmax - vmin
    n = len(series_list[0])

    def X(i):
        return int(pl + (W - pl - pr) * i / max(n - 1, 1))

    def Y(v):
        return int(H - pb - (H - pt - pb) * (v - vmin) / rng)

    cv2.line(img, (pl, pt), (pl, H - pb), (205, 205, 205), 1)
    cv2.line(img, (pl, H - pb), (W - pr, H - pb), (205, 205, 205), 1)
    for vx in vlines:
        cv2.line(img, (X(vx), pt), (X(vx), H - pb), (170, 170, 170), 1, cv2.LINE_AA)
    for s, c in zip(series_list, colors):
        pts = [(X(i), Y(float(v))) for i, v in enumerate(s)]
        for a, b in zip(pts, pts[1:]):
            cv2.line(img, a, b, c, 2, cv2.LINE_AA)
    for txt, y in (("%.0f" % vmax, pt + 8), ("%.0f" % vmin, H - pb)):
        cv2.putText(img, txt, (8, y), cv2.FONT_HERSHEY_SIMPLEX,
                    0.45, (90, 90, 90), 1, cv2.LINE_AA)
    for i, lg in enumerate(legends):
        cv2.putText(img, lg, (pl + 8 + i * 190, pt + 10), cv2.FONT_HERSHEY_SIMPLEX,
                    0.5, colors[i % len(colors)], 2, cv2.LINE_AA)
    return img


def main():
    ap = argparse.ArgumentParser(description="grab-release 视频验收（通用）")
    ap.add_argument("--pet-id", default="04-warm-brown-tabby")
    ap.add_argument("--video", default=None, help="默认 <assetDir>/12-拎起/03-视频/grab-release-原始.mp4")
    args = ap.parse_args()

    profile = load_profile(args.pet_id)
    crop = profile["crop"]
    asset_dir = profile["assetDir"].replace("\\", "/")
    video = args.video or os.path.join(BASE, asset_dir, "12-拎起", "03-视频", "grab-release-原始.mp4")
    out_dir = os.path.join(BASE, asset_dir, "12-拎起", "06-验收")
    os.makedirs(out_dir, exist_ok=True)

    print("宠物        : %s" % args.pet_id)
    print("视频        : %s" % video)
    print("crop box    : x=%d y=%d size=%d (source %d)" %
          (crop["x"], crop["y"], crop["size"], crop["sourceSize"]))

    anchor, anchor_video, warmup = locate_anchor(profile)
    print("锚点帧来源  : %s (warmup=%d -> 第 %d 帧)" %
          (os.path.basename(anchor_video), warmup, warmup))
    a_mask = green_mask(anchor)
    a_shape = shape_of(a_mask)
    a_color = mean_color(anchor, a_mask)

    cap = cv2.VideoCapture(video)
    if not cap.isOpened():
        raise IOError("cannot open " + video)
    fps = cap.get(cv2.CAP_PROP_FPS)
    frames, boxes = [], []
    while True:
        ok, f = cap.read()
        if not ok:
            break
        m = green_mask(f)
        st = shape_of(m)
        boxes.append(st)
        frames.append((f, m))
    cap.release()

    n = len(frames)
    print("视频参数    : %dx%d  fps=%.2f  帧数=%d  时长=%.2fs" %
          (frames[0][0].shape[1], frames[0][0].shape[0], fps, n, n / fps if fps else 0))

    widths = [b["bbox"][2] for b in boxes]
    tops = [b["bbox"][1] for b in boxes]
    heights = [b["bbox"][3] for b in boxes]

    i_lift = int(n * SEG_LIFT_END)
    i_hold = int(n * SEG_HOLD_END)
    i_drop = int(n * SEG_DROP_END)

    f0, m0 = frames[0]
    fl, ml = frames[-1]
    s0, sl = boxes[0], boxes[-1]

    print("\n=== 1. 首帧 vs 锚点 ===")
    print("  锚点 bbox=%s aspect=%s tail=%+.4f" % (a_shape["bbox"], a_shape["aspect"], a_shape["tailNorm"]))
    print("  首帧 bbox=%s aspect=%s tail=%+.4f" % (s0["bbox"], s0["aspect"], s0["tailNorm"]))
    print("  bbox 差 = %s   IoU = %.4f" %
          ([abs(p - q) for p, q in zip(s0["bbox"], a_shape["bbox"])],
           mask_iou(m0, a_mask)))

    print("\n=== 2. 末帧 vs 锚点（关键判据）===")
    diff = [abs(p - q) for p, q in zip(sl["bbox"], a_shape["bbox"])]
    iou_last = mask_iou(ml, a_mask)
    tail_same = (sl["tailNorm"] * a_shape["tailNorm"]) > 0
    print("  末帧 bbox=%s aspect=%s tail=%+.4f" % (sl["bbox"], sl["aspect"], sl["tailNorm"]))
    print("  bbox 差 = %s  (判据 < 5)" % diff)
    print("  IoU = %.4f  (判据 > 0.95)" % iou_last)
    print("  尾巴同侧 = %s  (锚点 %+.4f / 末帧 %+.4f)" %
          ("是" if tail_same else "【否】", a_shape["tailNorm"], sl["tailNorm"]))
    ok_last = max(diff) < 5 and iou_last > 0.95 and tail_same
    print("  => %s" % ("PASS" if ok_last else "FAIL（末帧未回到锚点）"))

    print("\n=== 3. 头部高度稳定性（验证没有下沉/位移）===")
    top_arr = np.array(tops)
    print("  y0  min=%d max=%d 波动=%d px  (判据 < 15)" %
          (top_arr.min(), top_arr.max(), top_arr.max() - top_arr.min()))
    print("  高度 min=%d max=%d 波动=%d px" %
          (min(heights), max(heights), max(heights) - min(heights)))
    ok_top = bool((top_arr.max() - top_arr.min()) < 15)
    print("  => %s" % ("PASS" if ok_top else "FAIL（存在上下位移）"))

    print("\n=== 4. 悬空段与循环点 ===")
    print("  全局 宽度 min=%d max=%d | 高度 min=%d max=%d" %
          (min(widths), max(widths), min(heights), max(heights)))
    min_w = min(widths)
    narrow = [i for i, w in enumerate(widths) if w <= min_w * 1.25]
    if narrow:
        segs, cur = [], [narrow[0]]
        for i in narrow[1:]:
            if i == cur[-1] + 1:
                cur.append(i)
            else:
                segs.append(cur)
                cur = [i]
        segs.append(cur)
        best = max(segs, key=len)
        m_lift, m_hold = best[0], best[-1]
    else:
        m_lift, m_hold = i_lift, i_hold
    print("  设计区间 f%04d ~ f%04d（%.0f%% ~ %.0f%%）" %
          (i_lift, i_hold, SEG_LIFT_END * 100, SEG_HOLD_END * 100))
    print("  实测最窄段 f%04d ~ f%04d（%d 帧）宽度 %d~%d" %
          (m_lift, m_hold, m_hold - m_lift + 1,
           min(widths[m_lift:m_hold + 1]), max(widths[m_lift:m_hold + 1])))
    hold_w = widths[m_lift:m_hold + 1]
    print("  悬空段宽度波动 = %d px" % (max(hold_w) - min(hold_w)))
    iou_loop = mask_iou(frames[m_lift][1], frames[m_hold][1])
    print("  循环点 IoU(f%04d vs f%04d) = %.4f  (判据 > 0.9)" % (m_lift, m_hold, iou_loop))
    ok_loop = bool(iou_loop > 0.9)
    print("  => %s" % ("PASS" if ok_loop else "WARN（循环点可能跳变）"))
    hold_range = {"measured": [m_lift, m_hold], "design": [i_lift, i_hold]}

    print("\n=== 5. 越界检查（crop box）===")
    x0c, y0c = crop["x"], crop["y"]
    x1c, y1c = crop["x"] + crop["size"] - 1, crop["y"] + crop["size"] - 1
    worst = {"left": 0, "top": 0, "right": 0, "bottom": 0}
    bad = 0
    worst_frame, worst_sum = -1, 0
    for idx, b in enumerate(boxes):
        bx, by, bw, bh = b["bbox"]
        o = {"left": max(0, x0c - bx), "top": max(0, y0c - by),
             "right": max(0, (bx + bw - 1) - x1c), "bottom": max(0, (by + bh - 1) - y1c)}
        s = int(sum(o.values()))
        if s > 0:
            bad += 1
            for k in worst:
                worst[k] = max(worst[k], o[k])
            if s > worst_sum:
                worst_sum, worst_frame = s, idx
    print("  越界帧 %d/%d   最大越界 左%d 上%d 右%d 下%d" %
          (bad, n, worst["left"], worst["top"], worst["right"], worst["bottom"]))
    if worst_frame >= 0:
        print("  最严重帧 f%04d（合计越界 %d px）" % (worst_frame, worst_sum))
    ok_crop = bool(bad == 0)
    print("  => %s" % ("PASS" if ok_crop else "FAIL（超出 crop box）"))

    print("\n=== 6. 毛色 drift ===")
    c0 = mean_color(f0, m0)
    cl = mean_color(fl, ml)
    d_rgb = [round(abs(p - q), 1) for p, q in zip(cl, a_color)]
    print("  锚点 RGB=%s" % a_color)
    print("  首帧 RGB=%s   差=%s" % (c0, [round(abs(p - q), 1) for p, q in zip(c0, a_color)]))
    print("  末帧 RGB=%s   差=%s" % (cl, d_rgb))
    ok_color = max(d_rgb) < 12
    print("  => %s (判据 单通道差 < 12)" % ("PASS" if ok_color else "WARN"))

    # ---- 出图 ----
    vlines = [i_lift, i_hold, i_drop]
    p1 = draw_plot([widths], [(60, 90, 200)], vlines,
                   legends=("bbox width",))
    p2 = draw_plot([tops], [(50, 140, 60)], vlines,
                   legends=("bbox top y",))
    imwrite_any(os.path.join(out_dir, "曲线-bbox宽度.png"), p1)
    imwrite_any(os.path.join(out_dir, "曲线-头部高度.png"), p2)

    picks = sorted({0, i_lift, (i_lift + i_hold) // 2, i_hold,
                    (i_hold + i_drop) // 2, i_drop, n - 1})
    tiles = []
    for i in picks:
        f, m = frames[i]
        vis = f.copy()
        ys, xs = np.nonzero(m)
        cv2.rectangle(vis, (int(xs.min()), int(ys.min())),
                      (int(xs.max()), int(ys.max())), (0, 0, 255), 3)
        p0c = (crop["x"], crop["y"])
        p1c = (crop["x"] + crop["size"] - 1, crop["y"] + crop["size"] - 1)
        cv2.rectangle(vis, p0c, p1c, (0, 190, 0), 2)
        cv2.putText(vis, "f%04d" % i, (14, 40), cv2.FONT_HERSHEY_SIMPLEX,
                    1.1, (0, 0, 255), 3, cv2.LINE_AA)
        tiles.append(cv2.resize(vis, (300, 300)))
    rows = [np.hstack(tiles[i:i + 4]) for i in range(0, len(tiles), 4)]
    while len(rows[-1].shape) and rows[-1].shape[1] < rows[0].shape[1]:
        pad = np.full((300, rows[0].shape[1] - rows[-1].shape[1], 3), 255, np.uint8)
        rows[-1] = np.hstack([rows[-1], pad])
    imwrite_any(os.path.join(out_dir, "关键帧拼图.png"), np.vstack(rows))

    H2 = 520
    def prep(img, m):
        rgb = cv2.cvtColor(img[:, :, :3], cv2.COLOR_BGR2RGB)
        out = np.where(m[:, :, None], rgb, 250)
        sc = H2 / float(out.shape[0])
        return cv2.resize(out, (int(out.shape[1] * sc), H2))

    left = prep(anchor, a_mask)
    right = prep(fl, ml)
    canvas = np.hstack([left, np.full((H2, 24, 3), 255, np.uint8), right])
    bar = np.full((40, canvas.shape[1], 3), 255, np.uint8)
    cv2.putText(bar, "anchor (idle prototype)", (16, 27),
                cv2.FONT_HERSHEY_SIMPLEX, 0.72, (20, 20, 20), 2, cv2.LINE_AA)
    cv2.putText(bar, "last frame  IoU=%.3f" % iou_last, (left.shape[1] + 40, 27),
                cv2.FONT_HERSHEY_SIMPLEX, 0.72, (20, 20, 20), 2, cv2.LINE_AA)
    imwrite_any(os.path.join(out_dir, "末帧vs锚点.png"), np.vstack([canvas, bar]))

    report = {
        "petId": args.pet_id, "video": video, "fps": fps, "frameCount": n,
        "crop": crop, "segments": {"liftEnd": i_lift, "holdEnd": i_hold, "dropEnd": i_drop},
        "anchorShape": a_shape, "firstShape": s0, "lastShape": sl,
        "bboxDiffLastVsAnchor": diff, "iouLastVsAnchor": iou_last,
        "tailSameSide": bool(tail_same),
        "topYRange": [int(top_arr.min()), int(top_arr.max())],
        "heightRange": [int(min(heights)), int(max(heights))],
        "loopIou": iou_loop,
        "overscan": {"badFrames": bad, "worst": worst},
        "colorAnchor": a_color, "colorFirst": c0, "colorLast": cl,
        "verdict": {"lastFrame": ok_last, "headStable": ok_top,
                    "loop": ok_loop, "noOverscan": ok_crop, "color": ok_color},
    }
    with open(os.path.join(out_dir, "验收.json"), "w", encoding="utf-8") as f:
        json.dump(report, f, ensure_ascii=False, indent=2)

    print("\n总结: 末帧 %s | 头部稳定 %s | 循环点 %s | 越界 %s | 毛色 %s" %
          ("PASS" if ok_last else "FAIL", "PASS" if ok_top else "FAIL",
           "PASS" if ok_loop else "WARN", "PASS" if ok_crop else "FAIL",
           "PASS" if ok_color else "WARN"))
    print("输出目录: " + out_dir)


if __name__ == "__main__":
    main()
