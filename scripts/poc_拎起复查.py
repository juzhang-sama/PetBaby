# -*- coding: utf-8 -*-
"""poc_拎起复查.py —— grab-release 两支新视频的三项复查。

老王提的三个问题：
  Q1 视频里出现了一只人手 —— 是不是真的人手？在哪几帧？多大？
  Q2 拎起时宠物有部分内容出画 —— 是超出 640 画幅（无救）还是超出 crop box（会裁）？
  Q3 建国(05) 和 毛砌墙(04) 拎起方式不同 —— 差在哪？正常么？

判据：
  出画-A：bbox 触到 640 画幅边缘（内容真被切，无法补救）
  出画-B：bbox 超出各自 idle crop box（抠像后会被裁掉）
  人手  ：绿幕背景里出现「肤色 + 大面积连通域 + 触达画面边缘」的物体
          肤色用 YCrCb + HSV 双通道交集降噪；暖棕虎斑会误报，所以必须
          同时满足「连通域够大」且「触到画面边缘」，并出证据图让人眼复核。

用法：
  python poc_拎起复查.py            # 两只宠物都跑
  python poc_拎起复查.py --pet-id 04-warm-brown-tabby
"""
import os
import sys
import json
import argparse
import numpy as np
import cv2

BASE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PROFILE_DIR = os.path.join(BASE, "output", "宠物档案")

PETS = {
    "04-warm-brown-tabby": {
        "short": "04毛砌墙",
        "video": os.path.join(BASE, "output", "宠物动作-毛砌墙-v3-2026-08-31",
                              "12-拎起", "03-视频", "grab-release-原始-v2.mp4"),
        "out": os.path.join(BASE, "output", "宠物动作-毛砌墙-v3-2026-08-31",
                            "12-拎起", "04-速诊"),
    },
    "05-silver-tabby": {
        "short": "05建国",
        "video": os.path.join(BASE, "output", "宠物动作-建国-v1-2026-09-03",
                              "12-拎起", "03-视频", "grab-release-原始-v2.mp4"),
        "out": os.path.join(BASE, "output", "宠物动作-建国-v1-2026-09-03",
                            "12-拎起", "04-速诊"),
    },
}


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


def alpha_mask(img):
    if img.ndim == 3 and img.shape[2] == 4:
        return img[:, :, 3] > 128
    return green_mask(img[:, :, :3])


def skin_mask(bgr):
    """肤色检测：YCrCb 与 HSV 取交集，降噪。"""
    ycrcb = cv2.cvtColor(bgr, cv2.COLOR_BGR2YCrCb)
    m1 = cv2.inRange(ycrcb, (0, 133, 77), (255, 173, 127))
    hsv = cv2.cvtColor(bgr, cv2.COLOR_BGR2HSV)
    m2 = cv2.inRange(hsv, (0, 40, 60), (25, 170, 255))
    m3 = cv2.inRange(hsv, (165, 40, 60), (180, 170, 255))  # 红端回绕
    return cv2.bitwise_and(m1, cv2.bitwise_or(m2, m3))


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
        "bbox": [x0, y0, w, h],
        "aspect": round(w / h, 3),
        "area": int(mask.sum()),
        "centerX": round(cx, 1),
        "centerY": round(float(ys.mean()), 1),
        "tailNorm": round((cx_low - cx) / w, 4),
    }


def restore_to_640(img, crop):
    """把 588 的锚点帧按 crop 参数还原回 640 原画幅。"""
    size = crop["size"]
    rs = cv2.resize(img, (size, size), interpolation=cv2.INTER_LINEAR)
    c = np.zeros((640, 640, rs.shape[2]), np.uint8)
    c[crop["y"]:crop["y"] + size, crop["x"]:crop["x"] + size] = rs
    return c


def scan(video_path):
    """逐帧扫：bbox / 出画 / 肤色团块。"""
    cap = cv2.VideoCapture(video_path)
    if not cap.isOpened():
        raise IOError("cannot open: " + video_path)
    fps = cap.get(cv2.CAP_PROP_FPS)
    w = int(cap.get(cv2.CAP_PROP_FRAME_WIDTH))
    h = int(cap.get(cv2.CAP_PROP_FRAME_HEIGHT))
    rows, kept, i = [], {}, 0
    while True:
        ok, f = cap.read()
        if not ok:
            break
        m = green_mask(f)
        st = shape_of(m) or {"bbox": [0, 0, 0, 0], "aspect": 0, "area": 0,
                             "centerX": 0, "centerY": 0, "tailNorm": 0}
        # 肤色团块（限定在非绿前景里找，绿幕本身不可能有肤色）
        sk = skin_mask(f)
        sk = cv2.bitwise_and(sk, m.astype(np.uint8) * 255)
        sk = cv2.morphologyEx(sk, cv2.MORPH_OPEN, np.ones((5, 5), np.uint8))
        n, lab, stats, cent = cv2.connectedComponentsWithStats(sk, 8)
        best = None
        for k in range(1, n):
            a = int(stats[k, cv2.CC_STAT_AREA])
            if a < 600:
                continue
            bx = int(stats[k, cv2.CC_STAT_LEFT])
            by = int(stats[k, cv2.CC_STAT_TOP])
            bw = int(stats[k, cv2.CC_STAT_WIDTH])
            bh = int(stats[k, cv2.CC_STAT_HEIGHT])
            touch_edge = (bx <= 2 or by <= 2 or bx + bw >= w - 3 or by + bh >= h - 3)
            cand = dict(area=a, box=[bx, by, bw, bh], touchEdge=bool(touch_edge))
            if best is None or a > best["area"]:
                best = cand
        rows.append(dict(i=i, shape=st, skin=best,
                         skinPx=int((sk > 0).sum())))
        if i in (0, 9, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 119, 120):
            kept[i] = (f, m, sk)
        i += 1
    cap.release()
    return {"fps": round(fps, 2), "w": w, "h": h, "count": len(rows),
            "rows": rows, "kept": kept}


def overscan_report(rows, crop):
    """返回 画幅越界 与 crop box 越界 两套统计。"""
    x0c, y0c = crop["x"], crop["y"]
    x1c, y1c = crop["x"] + crop["size"] - 1, crop["y"] + crop["size"] - 1
    frame_bad, crop_bad = [], []
    worst_f = {"left": 0, "top": 0, "right": 0, "bottom": 0}
    worst_c = {"left": 0, "top": 0, "right": 0, "bottom": 0}
    for r in rows:
        bx, by, bw, bh = r["shape"]["bbox"]
        bx1, by1 = bx + bw - 1, by + bh - 1
        of = {"left": max(0, 0 - bx), "top": max(0, 0 - by),
              "right": max(0, bx1 - 639), "bottom": max(0, by1 - 639)}
        oc = {"left": max(0, x0c - bx), "top": max(0, y0c - by),
              "right": max(0, bx1 - x1c), "bottom": max(0, by1 - y1c)}
        if any(v > 0 for v in of.values()):
            frame_bad.append(dict(frame=r["i"], **of))
        if any(v > 0 for v in oc.values()):
            crop_bad.append(dict(frame=r["i"], **oc))
        for k in worst_f:
            worst_f[k] = max(worst_f[k], of[k])
        for k in worst_c:
            worst_c[k] = max(worst_c[k], oc[k])
    return {"worstFrame": worst_f, "frameBadCount": len(frame_bad),
            "frameBad": frame_bad[:40],
            "worstCrop": worst_c, "cropBadCount": len(crop_bad),
            "cropBad": crop_bad[:40]}


def draw_overlay(bgr, mask, sk, label, crop=None, size=300):
    vis = cv2.resize(bgr, (size, size))
    s = size / float(bgr.shape[0])
    if sk is not None and sk.sum() > 0:
        ys, xs = np.nonzero(cv2.resize(sk, (size, size)) > 0)
        if len(xs):
            # 肤色区域染成品红，便于一眼看见人手
            vis[ys, xs] = (255, 0, 255)
    if crop is not None:
        p0 = (int(crop["x"] * s), int(crop["y"] * s))
        p1 = (int((crop["x"] + crop["size"] - 1) * s),
              int((crop["y"] + crop["size"] - 1) * s))
        cv2.rectangle(vis, p0, p1, (0, 190, 0), 2)
    if mask is not None and mask.sum() > 0:
        mk = cv2.resize(mask.astype(np.uint8) * 255, (size, size)) > 127
        ys, xs = np.nonzero(mk)
        cv2.rectangle(vis, (int(xs.min()), int(ys.min())),
                      (int(xs.max()), int(ys.max())), (0, 0, 255), 3)
    cv2.putText(vis, label, (12, 34), cv2.FONT_HERSHEY_SIMPLEX,
                0.95, (0, 0, 255), 3, cv2.LINE_AA)
    return vis


def build_strip(kept, crop, per_row=4, size=280):
    idxs = sorted(kept.keys())
    rows = []
    for r in range(0, len(idxs), per_row):
        cells = []
        for i in idxs[r:r + per_row]:
            f, m, sk = kept[i]
            cells.append(draw_overlay(f, m, sk, "f%04d" % i, crop, size))
        while len(cells) < per_row:
            cells.append(np.full((size, size, 3), 250, np.uint8))
        rows.append(np.hstack(cells))
    return np.vstack(rows)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pet-id", default=None, help="只跑某一只宠物")
    ap.add_argument("--version", default="v5",
                    help="视频版本号，对应 grab-release-原始-<version>.mp4（默认 v5，2026-09-05 拍板版）")
    args = ap.parse_args()

    for pid, cfg in PETS.items():
        cfg = dict(cfg)
        cfg["video"] = os.path.join(os.path.dirname(cfg["video"]),
                                    "grab-release-原始-%s.mp4" % args.version)
        cfg["out"] = os.path.join(cfg["out"], args.version)
        PETS[pid] = cfg
    if not os.path.exists(list(PETS.values())[0]["video"]):
        print("[错误] 找不到视频 %s（--version=%s）"
              % (list(PETS.values())[0]["video"], args.version))
        return

    pets = PETS if not args.pet_id else {args.pet_id: PETS[args.pet_id]}
    summary = {}

    for pid, cfg in pets.items():
        prof = json.load(open(os.path.join(PROFILE_DIR, pid + ".json"), encoding="utf-8"))
        crop = prof["crop"]
        anchor = imread_any(os.path.join(BASE, prof["runtime"]["anchorFrame"]))
        anchor640 = restore_to_640(anchor, crop)
        am = alpha_mask(anchor640)
        ash = shape_of(am)

        print("\n" + "=" * 78)
        print("【%s】%s" % (cfg["short"], pid))
        print("=" * 78)
        print("  视频: %s" % cfg["video"])
        print("  idle crop box: x=%d y=%d size=%d   锚点还原640 bbox=%s aspect=%s"
              % (crop["x"], crop["y"], crop["size"], ash["bbox"], ash["aspect"]))

        info = scan(cfg["video"])
        rows = info["rows"]
        print("  分辨率 %dx%d  fps=%s  帧数=%d  时长=%.2fs"
              % (info["w"], info["h"], info["fps"], info["count"],
                 info["count"] / info["fps"] if info["fps"] else 0))

        first, last = rows[0], rows[-1]
        fs, ls = first["shape"], last["shape"]
        print("\n  --- 首尾一致性（vs idle 锚点）---")
        print("  锚点: bbox=%s aspect=%s" % (ash["bbox"], ash["aspect"]))
        print("  首帧: bbox=%s aspect=%s area=%d" % (fs["bbox"], fs["aspect"], fs["area"]))
        print("  末帧: bbox=%s aspect=%s area=%d" % (ls["bbox"], ls["aspect"], ls["area"]))
        d_first = max(abs(fs["bbox"][k] - ash["bbox"][k]) for k in range(4))
        d_last = max(abs(ls["bbox"][k] - ash["bbox"][k]) for k in range(4))
        print("  bbox 最大分量差: 首帧 %dpx  末帧 %dpx" % (d_first, d_last))

        # ---- Q2 出画 ----
        ov = overscan_report(rows, crop)
        print("\n  --- Q2 出画 ---")
        print("  [画幅 640] 越界帧 %d/%d  最大 左%d 上%d 右%d 下%d"
              % (ov["frameBadCount"], len(rows),
                 ov["worstFrame"]["left"], ov["worstFrame"]["top"],
                 ov["worstFrame"]["right"], ov["worstFrame"]["bottom"]))
        print("  [crop box] 越界帧 %d/%d  最大 左%d 上%d 右%d 下%d"
              % (ov["cropBadCount"], len(rows),
                 ov["worstCrop"]["left"], ov["worstCrop"]["top"],
                 ov["worstCrop"]["right"], ov["worstCrop"]["bottom"]))
        if ov["cropBad"]:
            wf = max(ov["cropBad"],
                     key=lambda b: max(b["left"], b["top"], b["right"], b["bottom"]))
            print("    最严重 f%04d: 左%d 上%d 右%d 下%d"
                  % (wf["frame"], wf["left"], wf["top"], wf["right"], wf["bottom"]))

        # ---- Q1 人手 ----
        skin_frames = [r for r in rows if r["skin"] and r["skin"]["area"] >= 1500]
        edge_frames = [r for r in skin_frames if r["skin"]["touchEdge"]]
        maxskin = max(rows, key=lambda r: r["skin"]["area"] if r["skin"] else 0)
        print("\n  --- Q1 人手 ---")
        print("  肤色团块 >=1500px 的帧: %d/%d" % (len(skin_frames), len(rows)))
        print("  其中团块触到画面边缘的帧: %d" % len(edge_frames))
        if maxskin["skin"]:
            print("  最大团块 f%04d: 面积=%d  bbox=%s  触边=%s"
                  % (maxskin["i"], maxskin["skin"]["area"],
                     maxskin["skin"]["box"], maxskin["skin"]["touchEdge"]))
        if skin_frames:
            print("  帧号分布: %s" %
                  " ".join(str(r["i"]) for r in skin_frames[:40]))

        # ---- Q3 拎起姿态轨迹 ----
        widths = [r["shape"]["bbox"][2] for r in rows]
        heights = [r["shape"]["bbox"][3] for r in rows]
        tops = [r["shape"]["bbox"][1] for r in rows]
        cxs = [r["shape"]["centerX"] for r in rows]
        narrow = min(range(len(rows)), key=lambda k: widths[k])
        tallest = min(range(len(rows)), key=lambda k: tops[k])
        print("\n  --- Q3 拎起姿态 ---")
        print("  宽: 首%d 最窄%d(f%04d, %.2fs) 末%d   收窄到 %.0f%%"
              % (widths[0], widths[narrow], narrow, narrow / info["fps"],
                 widths[-1], widths[narrow] / widths[0] * 100))
        print("  高: 首%d 最高%d(f%04d) 末%d"
              % (heights[0], heights[tallest], tallest, heights[-1]))
        print("  顶: 首%d 最高%d(f%04d, 抬升 %dpx) 末%d"
              % (tops[0], tops[tallest], tallest, tops[0] - tops[tallest], tops[-1]))
        print("  质心x: 首%.0f 最窄帧%.0f 末%.0f  横移 %.0fpx"
              % (cxs[0], cxs[narrow], cxs[-1], max(cxs) - min(cxs)))
        print("  尾巴 tailNorm: 首%+.4f 最窄帧%+.4f 末%+.4f"
              % (rows[0]["shape"]["tailNorm"], rows[narrow]["shape"]["tailNorm"],
                 rows[-1]["shape"]["tailNorm"]))

        # 悬空段：宽度接近最窄的连续区间（供运行时 hold 循环用）
        thr = widths[narrow] * 1.15
        hold = [i for i, wd in enumerate(widths) if wd <= thr]
        hold_lo = min(hold) if hold else None
        hold_hi = max(hold) if hold else None
        print("  悬空段(宽 <= 最窄*1.15): f%04d ~ f%04d  共 %d 帧"
              % (hold_lo or -1, hold_hi or -1, len(hold)))

        os.makedirs(cfg["out"], exist_ok=True)
        imwrite_any(os.path.join(cfg["out"], "复查-关键帧.png"),
                    build_strip(info["kept"], crop))
        summary[pid] = {
            "short": cfg["short"],
            "video": cfg["video"],
            "fps": info["fps"], "frameCount": info["count"],
            "durationSec": round(info["count"] / info["fps"], 2) if info["fps"] else None,
            "anchor": {"bbox": ash["bbox"], "aspect": ash["aspect"]},
            "first": fs, "last": ls,
            "bboxDiffFirst": int(d_first), "bboxDiffLast": int(d_last),
            "overscan": ov,
            "skin": {
                "framesWithBlob": len(skin_frames),
                "framesTouchEdge": len(edge_frames),
                "frameList": [r["i"] for r in skin_frames],
                "maxBlob": maxskin["skin"],
                "maxBlobFrame": maxskin["i"],
            },
            "pose": {
                "widths": widths, "heights": heights, "tops": tops,
                "narrowestFrame": int(narrow),
                "narrowestWidth": int(widths[narrow]),
                "narrowRatio": round(widths[narrow] / widths[0], 3),
                "liftPx": int(tops[0] - tops[tallest]),
                "liftFrame": int(tallest),
                "xShift": round(max(cxs) - min(cxs), 1),
                "holdRange": [hold_lo, hold_hi],
                "tailNorm": [rows[0]["shape"]["tailNorm"],
                             rows[narrow]["shape"]["tailNorm"],
                             rows[-1]["shape"]["tailNorm"]],
            },
        }
        with open(os.path.join(cfg["out"], "复查.json"), "w", encoding="utf-8") as f:
            json.dump(summary, f, ensure_ascii=False, indent=2)

    # ---- 两只猫差异对比 ----
    if len(summary) == 2:
        a, b = list(summary.values())
        print("\n" + "=" * 78)
        print("【Q3 两只猫拎起方式差异】")
        print("=" * 78)
        pa, pb = a["pose"], b["pose"]
        print("  %-10s 最窄宽度 %d (%d%%)   抬升 %dpx   横移 %.0fpx   悬空段 f%d~f%d"
              % (a["short"], pa["narrowestWidth"], pa["narrowRatio"] * 100,
                 pa["liftPx"], pa["xShift"], pa["holdRange"][0], pa["holdRange"][1]))
        print("  %-10s 最窄宽度 %d (%d%%)   抬升 %dpx   横移 %.0fpx   悬空段 f%d~f%d"
              % (b["short"], pb["narrowestWidth"], pb["narrowRatio"] * 100,
                 pb["liftPx"], pb["xShift"], pb["holdRange"][0], pb["holdRange"][1]))
        print("  首帧宽度: %s=%d  %s=%d" %
              (a["short"], pa["widths"][0], b["short"], pb["widths"][0]))
        print("  人手: %s %d帧 / %s %d帧"
              % (a["short"], a["skin"]["framesWithBlob"],
                 b["short"], b["skin"]["framesWithBlob"]))
        print("  crop 越界: %s %d帧 / %s %d帧"
              % (a["short"], a["overscan"]["cropBadCount"],
                 b["short"], b["overscan"]["cropBadCount"]))

    print("\n输出: 各宠物 12-拎起/04-速诊/复查-关键帧.png + 复查.json")


if __name__ == "__main__":
    main()
