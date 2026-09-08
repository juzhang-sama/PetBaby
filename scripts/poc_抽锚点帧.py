# -*- coding: utf-8 -*-
"""poc_抽锚点帧.py —— 从 idle 原始视频抽取锚点帧（首尾帧素材），通用。

从宠物档案 -> 抠像参数（runtime.refParams）反查 idle 原始视频路径与 warmupFramesSkipped，
按 warmup 抽取锚点帧（对应 frames/idle-combo/f0000 的绿幕原图），保存 PNG。

用法：
  D:/DevTools/Python312/python.exe scripts/poc_抽锚点帧.py --pet-id 04-warm-brown-tabby
  D:/DevTools/Python312/python.exe scripts/poc_抽锚点帧.py --pet-id 05-silver-tabby --size 640
"""
import os
import json
import argparse
import cv2

BASE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def imwrite_any(path, img):
    ok, buf = cv2.imencode(os.path.splitext(path)[1], img)
    if not ok:
        raise IOError("imencode failed: " + path)
    buf.tofile(path)


def main():
    ap = argparse.ArgumentParser(description="抽取 idle 锚点帧（首尾帧素材，通用）")
    ap.add_argument("--pet-id", required=True)
    ap.add_argument("--size", type=int, default=1024, help="输出边长，默认 1024")
    ap.add_argument("--out-dir", default=None, help="默认 <assetDir>/12-拎起/05-尾帧素材")
    args = ap.parse_args()

    prof_path = os.path.join(BASE, "output", "宠物档案", args.pet_id + ".json")
    with open(prof_path, encoding="utf-8") as f:
        profile = json.load(f)

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
        raise SystemExit("[失败] 无法从 %s 读第 %d 帧" % (video, warmup))

    asset_dir = profile["assetDir"].replace("\\", "/")
    out_dir = args.out_dir or os.path.join(BASE, asset_dir, "12-拎起", "05-尾帧素材")
    os.makedirs(out_dir, exist_ok=True)

    src = (frame.shape[1], frame.shape[0])
    if frame.shape[1] != args.size:
        frame = cv2.resize(frame, (args.size, args.size), interpolation=cv2.INTER_LANCZOS4)
    out = os.path.join(out_dir, "idle锚点帧-%d.png" % args.size)
    imwrite_any(out, frame)

    print("[锚点帧] %s" % out)
    print("[来源]   %s 第 %d 帧 (warmup=%d)  原始 %dx%d -> %dx%d" %
          (video, warmup, warmup, src[0], src[1], args.size, args.size))


if __name__ == "__main__":
    main()
