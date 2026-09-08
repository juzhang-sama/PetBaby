# -*- coding: utf-8 -*-
"""poc_拎起视频速诊.py —— carried / landed 两支视频的入口速诊。

判四件事（老王最关心第 4 条）：
1. 哪支是 carried（全程拎起）、哪支是 landed（首帧拎起、末帧坐姿）
2. f0000 有没有异常特写 / 构图跳变（Seedance 已知坑：首帧可能不可信）
3. 实测绿幕参考色（每个视频都要测，不能沿用旧常量）
4. landed 末帧的坐姿 vs idle-combo 锚点帧差多少——宽高比、尾巴朝向

输出：04-速诊/ 下的拼图 + 速诊.json
"""
import os
import json
import numpy as np
import cv2

BASE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PET = os.path.join(BASE, "output", "宠物动作-毛砌墙-v3-2026-08-31")
VID_DIR = os.path.join(PET, "12-拎起", "03-视频")
OUT_DIR = os.path.join(PET, "12-拎起", "04-速诊")
IDLE_FRAME = os.path.join(BASE, "apps", "desktop", "public", "builtin-pets",
                          "04-warm-brown-tabby", "frames", "idle-combo", "f0000.webp")


def imread_any(path):
    return cv2.imdecode(np.fromfile(path, dtype=np.uint8), cv2.IMREAD_UNCHANGED)


def imwrite_any(path, img):
    ok, buf = cv2.imencode(os.path.splitext(path)[1], img)
    if not ok:
        raise IOError("imencode failed: " + path)
    buf.tofile(path)


def green_mask(bgr, thresh=40):
    """非绿 = True。thresh 与项目抠像脚本口径一致。"""
    b = bgr[:, :, 0].astype(np.int16)
    g = bgr[:, :, 1].astype(np.int16)
    r = bgr[:, :, 2].astype(np.int16)
    return (g - np.maximum(r, b)) < thresh


def alpha_mask(img):
    if img.ndim == 3 and img.shape[2] == 4:
        return img[:, :, 3] > 128
    return green_mask(img[:, :, :3])


def sample_bg(bgr):
    """实测绿幕参考色：取四边 8px 条带的中位数（大部分是背景）。"""
    strip = np.concatenate([
        bgr[:8, :, :].reshape(-1, 3),
        bgr[-8:, :, :].reshape(-1, 3),
        bgr[:, :8, :].reshape(-1, 3),
        bgr[:, -8:, :].reshape(-1, 3),
    ])
    return [int(v) for v in np.median(strip, axis=0)[::-1]]  # BGR -> RGB


def shape_of(mask):
    """轮廓形态：bbox、宽高比、填充率、尾巴朝向。

    尾巴朝向用「最下方 25% 区域的质心 x - 整体质心 x」的相对量表示，
    归一化到 bbox 宽度后可跨画幅比较。正 = 尾巴偏右，负 = 尾巴偏左。
    """
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
        "bbox": [x0, y0, w, h],
        "aspect": round(w / h, 3),
        "fillRatio": round(float(mask.sum()) / mask.size, 4),
        "centerX": round(cx, 1),
        "tailNorm": round((cx_low - cx) / w, 4),
    }


def mask_iou(a, b):
    inter = np.logical_and(a, b).sum()
    union = np.logical_or(a, b).sum()
    return round(float(inter) / float(union), 4) if union else 0.0


def read_keyframes(path):
    """全帧扫 bbox（判越界），但只把关键帧图像留在内存里。"""
    cap = cv2.VideoCapture(path)
    if not cap.isOpened():
        raise IOError("cannot open: " + path)
    fps = cap.get(cv2.CAP_PROP_FPS)
    n = int(cap.get(cv2.CAP_PROP_FRAME_COUNT))
    w = int(cap.get(cv2.CAP_PROP_FRAME_WIDTH))
    h = int(cap.get(cv2.CAP_PROP_FRAME_HEIGHT))
    keep = sorted({0, min(9, max(n - 1, 0)), n // 4, n // 2, (3 * n) // 4, max(n - 1, 0)})
    kept, boxes, i, bg = {}, [], 0, None
    while True:
        ok, f = cap.read()
        if not ok:
            break
        m = green_mask(f)
        st = shape_of(m)
        boxes.append(dict(i=i, **st))
        if bg is None:
            bg = sample_bg(f)
        if i in keep:
            kept[i] = f
        i += 1
    cap.release()
    return {"fps": round(fps, 2), "declared": n, "actual": len(boxes),
            "w": w, "h": h, "frames": kept, "boxes": boxes, "bg": bg}


def overscan(boxes, crop):
    """全帧检查是否超出 idle 的 crop box（硬失败判据）。"""
    x0c, y0c = crop["x"], crop["y"]
    x1c, y1c = crop["x"] + crop["size"] - 1, crop["y"] + crop["size"] - 1
    worst = {"left": 0, "top": 0, "right": 0, "bottom": 0}
    bad = []
    for b in boxes:
        bx, by, bw, bh = b["bbox"]
        over = {
            "left": max(0, x0c - bx),
            "top": max(0, y0c - by),
            "right": max(0, (bx + bw - 1) - x1c),
            "bottom": max(0, (by + bh - 1) - y1c),
        }
        if any(v > 0 for v in over.values()):
            bad.append(dict(frame=b["i"], **over))
            for k in worst:
                worst[k] = max(worst[k], over[k])
    return worst, bad


def motion_range(boxes):
    """主体在画面内的平移范围（判摆动幅度是否失控）。"""
    lefts = [b["bbox"][0] for b in boxes]
    rights = [b["bbox"][0] + b["bbox"][2] - 1 for b in boxes]
    tops = [b["bbox"][1] for b in boxes]
    bots = [b["bbox"][1] + b["bbox"][3] - 1 for b in boxes]
    widths = [b["bbox"][2] for b in boxes]
    return {
        "xShift": max(lefts) - min(lefts),
        "yShift": max(tops) - min(tops),
        "maxWidth": max(widths),
        "minLeft": min(lefts), "maxRight": max(rights),
        "minTop": min(tops), "maxBottom": max(bots),
        "avgWidth": round(float(np.mean(widths)), 1),
    }


def draw_overlay(bgr, mask, label, crop=None):
    """绿框 = idle crop box（可显示区），红框 = 猫的实际范围。红溢出绿 = 会被裁掉。"""
    cell = bgr.shape[0]
    s = cell / float(mask.shape[0])
    vis = bgr.copy()
    if crop is not None:
        p0 = (int(crop["x"] * s), int(crop["y"] * s))
        p1 = (int((crop["x"] + crop["size"] - 1) * s),
              int((crop["y"] + crop["size"] - 1) * s))
        cv2.rectangle(vis, p0, p1, (0, 190, 0), 2)
    ys, xs = np.nonzero(mask)
    if len(xs):
        cv2.rectangle(vis, (int(xs.min() * s), int(ys.min() * s)),
                      (int(xs.max() * s), int(ys.max() * s)), (0, 0, 255), 3)
    cv2.putText(vis, label, (14, 40), cv2.FONT_HERSHEY_SIMPLEX,
                1.1, (0, 0, 255), 3, cv2.LINE_AA)
    return vis


def build_strip(frames_map, cell=300, crop=None):
    idxs = sorted(frames_map.keys())
    rows = []
    for r in range(0, len(idxs), 3):
        row = []
        for i in idxs[r:r + 3]:
            bgr = frames_map[i]["bgr"]
            row.append(draw_overlay(cv2.resize(bgr, (cell, cell)),
                                    frames_map[i]["mask"], "f%04d" % i, crop))
        while len(row) < 3:
            row.append(np.full((cell, cell, 3), 255, np.uint8))
        rows.append(np.hstack(row))
    return np.vstack(rows)


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    with open(os.path.join(BASE, "output", "宠物档案", "04-warm-brown-tabby.json"),
              encoding="utf-8") as f:
        crop = json.load(f)["crop"]
    print("idle crop box: x=%d y=%d size=%d (640 空间)  contentUnion=%s"
          % (crop["x"], crop["y"], crop["size"], crop.get("contentUnion")))
    report = {"_crop": crop}

    for name in ("原始-A", "原始-B"):
        path = os.path.join(VID_DIR, name + ".mp4")
        if not os.path.exists(path):
            print("[跳过] 未找到 " + path)
            continue
        info = read_keyframes(path)
        print("\n=== %s ===  分辨率 %dx%d  fps=%s  帧数=%d  时长=%.2fs"
              % (name, info["w"], info["h"], info["fps"], info["actual"],
                 info["actual"] / info["fps"] if info["fps"] else 0))

        frames, shapes = {}, {}
        for i in sorted(info["frames"].keys()):
            bgr = info["frames"][i]
            mask = green_mask(bgr)
            st = shape_of(mask)
            frames[i] = {"bgr": bgr, "mask": mask}
            shapes[i] = st
            print("  f%04d  aspect=%-6s fill=%-7s tailNorm=%-8s bbox=%s"
                  % (i, st["aspect"], st["fillRatio"], st["tailNorm"], st["bbox"]))

        bg = info["bg"]
        first, last = min(frames), max(frames)
        iou_f0_f9 = (mask_iou(frames[first]["mask"], frames[min(9, last)]["mask"])
                     if min(9, last) != first else 1.0)

        worst, bad = overscan(info["boxes"], crop)
        mr = motion_range(info["boxes"])

        report[name] = {
            "resolution": [info["w"], info["h"]],
            "fps": info["fps"],
            "frameCount": info["actual"],
            "durationSec": round(info["actual"] / info["fps"], 2) if info["fps"] else None,
            "bgColorRGB": bg,
            "shapes": {str(k): v for k, v in shapes.items()},
            "iou_f0000_f0009": iou_f0_f9,
            "overscan": {"worst": worst, "badFrameCount": len(bad), "frames": bad[:40]},
            "motionRange": mr,
        }
        print("  实测绿幕 RGB = %s   f0000/f0009 IoU = %s" % (bg, iou_f0_f9))
        print("  首帧 aspect=%.3f  末帧 aspect=%.3f" %
              (shapes[first]["aspect"], shapes[last]["aspect"]))
        print("  【越界】越界帧 %d/%d   最大越界: 左%d 上%d 右%d 下%d"
              % (len(bad), len(info["boxes"]),
                 worst["left"], worst["top"], worst["right"], worst["bottom"]))
        if bad:
            wf = max(bad, key=lambda b: max(b["left"], b["top"], b["right"], b["bottom"]))
            print("    最严重 f%04d: 左%d 上%d 右%d 下%d" %
                  (wf["frame"], wf["left"], wf["top"], wf["right"], wf["bottom"]))
        print("  【运动】水平平移 %dpx（猫均宽 %.0f，合 %.2f 倍猫宽）  垂直平移 %dpx  最宽 %dpx"
              % (mr["xShift"], mr["avgWidth"], mr["xShift"] / mr["avgWidth"],
                 mr["yShift"], mr["maxWidth"]))

        imwrite_any(os.path.join(OUT_DIR, "关键帧-%s.png" % name),
                    build_strip(frames, crop=crop))
        imwrite_any(os.path.join(OUT_DIR, "末帧-%s.png" % name), frames[last]["bgr"])

    # 归属判定：carried 全程拎起（末帧仍是竖长条），landed 末帧回到坐姿（近方形）
    picked = {}
    for name, rep in report.items():
        if name.startswith("_") or "shapes" not in rep:
            continue
        sh = rep["shapes"]
        klast = str(max(int(k) for k in sh.keys()))
        kfirst = str(min(int(k) for k in sh.keys()))
        last, first = sh[klast], sh[kfirst]
        picked[name] = "landed" if last["aspect"] > first["aspect"] * 1.15 else "carried"
    print("\n=== 归属判定 ===")
    for name, role in picked.items():
        print("  %s -> %s" % (name, role))
        report[name]["role"] = role

    landed_name = [n for n, r in picked.items() if r == "landed"]
    if not landed_name or not os.path.exists(IDLE_FRAME):
        print("\n[warn] 缺少 landed 或锚点帧，跳过坐姿对比")
        with open(os.path.join(OUT_DIR, "速诊.json"), "w", encoding="utf-8") as f:
            json.dump(report, f, ensure_ascii=False, indent=2)
        return

    landed_name = landed_name[0]
    cap = cv2.VideoCapture(os.path.join(VID_DIR, landed_name + ".mp4"))
    last_bgr = None
    while True:
        ok, fr = cap.read()
        if not ok:
            break
        last_bgr = fr
    cap.release()

    idle = imread_any(IDLE_FRAME)
    idle_mask = alpha_mask(idle)
    landed_mask = green_mask(last_bgr)

    s_idle = shape_of(idle_mask)
    s_land = shape_of(landed_mask)
    print("\n=== landed 末帧 vs idle-combo 锚点帧（老王关注点）===")
    print("  idle   锚点: aspect=%s  fill=%s  tailNorm=%s  size=%s"
          % (s_idle["aspect"], s_idle["fillRatio"], s_idle["tailNorm"], idle.shape[:2]))
    print("  landed 末帧: aspect=%s  fill=%s  tailNorm=%s  size=%s"
          % (s_land["aspect"], s_land["fillRatio"], s_land["tailNorm"], last_bgr.shape[:2]))
    tail_flip = (s_idle["tailNorm"] * s_land["tailNorm"]) < 0
    print("  尾巴朝向: idle=%+.4f  landed=%+.4f  -> %s"
          % (s_idle["tailNorm"], s_land["tailNorm"],
             "【相反】" if tail_flip else "同侧"))
    print("  宽高比差: %.3f (idle) vs %.3f (landed) 相差 %.1f%%"
          % (s_idle["aspect"], s_land["aspect"],
             abs(s_land["aspect"] - s_idle["aspect"]) / s_idle["aspect"] * 100))

    report["_landedVsIdle"] = {
        "idle": s_idle, "landed": s_land,
        "tailOpposite": bool(tail_flip),
        "aspectDiffPct": round(abs(s_land["aspect"] - s_idle["aspect"])
                               / s_idle["aspect"] * 100, 1),
    }

    # 并排对比图（缩放到同高，白色背景便于看轮廓）
    H = 520
    def prep(img, mask):
        rgb = cv2.cvtColor(img[:, :, :3], cv2.COLOR_BGR2RGB) if img.shape[2] >= 3 else img
        out = np.where(mask[:, :, None], rgb, 250)
        sc = H / out.shape[0]
        return cv2.resize(out, (int(out.shape[1] * sc), H))

    left = prep(idle[:, :, :3] if idle.shape[2] >= 3 else idle, idle_mask)
    right = prep(last_bgr, landed_mask)
    gap = np.full((H, 24, 3), 255, np.uint8)
    canvas = np.hstack([left, gap, right])
    bar = np.full((56, canvas.shape[1], 3), 255, np.uint8)
    cv2.putText(bar, "idle-combo f0000 (prototype)", (16, 36),
                cv2.FONT_HERSHEY_SIMPLEX, 0.9, (20, 20, 20), 2, cv2.LINE_AA)
    cv2.putText(bar, "landed last frame", (left.shape[1] + 40, 36),
                cv2.FONT_HERSHEY_SIMPLEX, 0.9, (20, 20, 20), 2, cv2.LINE_AA)
    imwrite_any(os.path.join(OUT_DIR, "坐姿对比-landed末帧vs原型.png"),
                np.vstack([canvas, bar]))

    # ---- 体型对比：把 idle 锚点帧还原到 640 原画幅，才能和视频帧同尺度比大小 ----
    def restore_idle(img):
        size = crop["size"]
        rs = cv2.resize(img, (size, size), interpolation=cv2.INTER_LINEAR)
        canvas = np.zeros((640, 640, img.shape[2]), np.uint8)
        canvas[crop["y"]:crop["y"] + size, crop["x"]:crop["x"] + size] = rs
        return canvas

    idle640 = restore_idle(idle)
    m_idle640 = alpha_mask(idle640)
    s_idle640 = shape_of(m_idle640)

    carried_names = [n for n, r in picked.items() if r == "carried"]
    tiles, legend, size_rows = [], [], {}
    if carried_names:
        cap = cv2.VideoCapture(os.path.join(VID_DIR, carried_names[0] + ".mp4"))
        ok, carried_first = cap.read()
        cap.release()
        m_carried = green_mask(carried_first)
        s_carried = shape_of(m_carried)
    else:
        carried_first, m_carried, s_carried = None, None, None

    print("\n=== 体型对比（统一到 640 画幅，等效边长 = sqrt(面积)）===")
    items = [("idle 原型坐姿", idle640, m_idle640, s_idle640)]
    if carried_first is not None:
        items.append(("carried 首帧", carried_first, m_carried, s_carried))
    items.append(("landed 末帧", last_bgr, landed_mask, s_land))

    for label, img, m, st in items:
        a = int(m.sum())
        eq = a ** 0.5
        size_rows[label] = {"areaPx": a, "equivSide": round(eq, 1),
                            "bbox": st["bbox"], "aspect": st["aspect"],
                            "tailNorm": st["tailNorm"]}
        print("  %-16s 面积=%7d  等效边长=%6.1f  bbox=%-22s aspect=%s tail=%+.4f"
              % (label, a, eq, st["bbox"], st["aspect"], st["tailNorm"]))

    base = size_rows["idle 原型坐姿"]["equivSide"]
    for label in size_rows:
        r = size_rows[label]["equivSide"] / base
        size_rows[label]["scaleVsIdle"] = round(r, 3)
        print("     %-16s 相对 idle 体型 = %.1f%%" % (label, r * 100))
    report["_sizeCompare"] = size_rows

    H2 = 520
    def prep2(img, m):
        rgb = cv2.cvtColor(img[:, :, :3], cv2.COLOR_BGR2RGB)
        out = np.where(m[:, :, None], rgb, 250)
        sc = H2 / float(out.shape[0])
        return cv2.resize(out, (int(out.shape[1] * sc), H2))

    short = {"idle 原型坐姿": "idle prototype", "carried 首帧": "carried f0000",
             "landed 末帧": "landed last"}
    for label, img, m, st in items:
        tiles.append(prep2(img, m))
        legend.append("%s  eq%.0f  %d%%" %
                      (short[label], size_rows[label]["equivSide"],
                       round(size_rows[label]["scaleVsIdle"] * 100)))
    gap2 = np.full((H2, 18, 3), 255, np.uint8)
    row = tiles[0]
    for t in tiles[1:]:
        row = np.hstack([row, gap2, t])
    bar2 = np.full((40, row.shape[1], 3), 255, np.uint8)
    x = 12
    for t in legend:
        cv2.putText(bar2, t, (x, 27), cv2.FONT_HERSHEY_SIMPLEX,
                    0.72, (20, 20, 20), 2, cv2.LINE_AA)
        x += 300
    imwrite_any(os.path.join(OUT_DIR, "体型对比-idle原型-vs-carried-vs-landed.png"),
                np.vstack([row, bar2]))

    with open(os.path.join(OUT_DIR, "速诊.json"), "w", encoding="utf-8") as f:
        json.dump(report, f, ensure_ascii=False, indent=2)
    print("\n输出目录: " + OUT_DIR)


if __name__ == "__main__":
    main()
