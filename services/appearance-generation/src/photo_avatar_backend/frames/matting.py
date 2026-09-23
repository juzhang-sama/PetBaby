# -*- coding: utf-8 -*-
"""绿幕视频 → 自动抠像 → 透明 PNG 帧序列（chroma 色键路径）。

从 `scripts/poc_抠像.py` **原样搬进服务**（算法零改动）：脚本现在只是这里的薄 CLI 包装。

- 色键 + 反算去绿 + 形态学清理 + 空间/时间平滑（无需下载模型）
- 自动跳过开头背景未稳定的过渡帧（模型接管前的几帧轮廓偏小，会把面积波动顶高）
- autocrop：按「全部帧前景并集」裁正方形（Seedance 常把方图首帧铺进 16:9 画布）
- 可选 color-match：以母版 PNG 的主体颜色校正成品（修 Seedance 改色）

产物：`frames/f000.png …` + `抠像参数.json`。

⚠️ **`抠像参数.json` 的字段被别的脚本消费**，改字段前先查这三个：
`poc_一次性动作对齐.py`（读 `crop` / 输出边长）、`poc_动作单元体检.py`（读 `crop`）、
`poc_预览.py`（读 `method`）。
"""
from __future__ import annotations

import json
import shutil
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

import numpy as np
from PIL import Image
from scipy import ndimage

from ._paths import rel_to, sha256_of

FFMPEG = shutil.which("ffmpeg") or "ffmpeg"
FFPROBE = shutil.which("ffprobe") or "ffprobe"


# 绿幕参考色（必须与首帧合成时使用的 GREEN 一致）
GREEN_RGB = np.array([0.0, 255.0, 0.0])

# 色键阈值：G - max(R,B) 的软过渡区间
#   低于 LOW   -> 完全前景（猫身上的暗部、高光都远低于此值）
#   高于 HIGH  -> 完全背景（纯绿幕远高于此值）
KEY_LOW = 18.0
KEY_HIGH = 62.0

# 反算去绿的最小 alpha：低于此值时直接反算会放大噪声，改用温和去绿
UNMIX_MIN_ALPHA = 0.20

# 紧裁时给主体留的硬边距（像素）。留白会被裁掉，这个值只是防止边缘像素被切。
CROP_MARGIN = 12

# 判定"背景已稳定"的容差：单帧背景 G 与中位数相差超过这个值视为过渡帧
WARMUP_TOL = 12.0


def run(cmd: list[str]) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace")


def probe_fps(video: Path) -> float:
    result = run([FFPROBE, "-v", "error", "-select_streams", "v:0",
                  "-show_entries", "stream=r_frame_rate", "-of", "default=nw=1:nk=1",
                  str(video)])
    text = (result.stdout or "").strip()
    if "/" in text:
        num, _, den = text.partition("/")
        try:
            return float(num) / float(den) if float(den) else float(num)
        except ValueError:
            pass
    try:
        return float(text)
    except ValueError:
        return 24.0


def probe_size(video: Path) -> tuple[int, int]:
    result = run([FFPROBE, "-v", "error", "-select_streams", "v:0",
                  "-show_entries", "stream=width,height", "-of", "csv=s=x:p=0", str(video)])
    text = (result.stdout or "").strip()
    if "x" in text:
        w, _, h = text.partition("x")
        try:
            return int(w), int(h)
        except ValueError:
            pass
    return 0, 0


class MattingError(ValueError):
    """抠像失败。

    刻意继承 `ValueError` 而不是抛 `SystemExit`：**`SystemExit` 不是 `Exception`**，
    服务侧 `job_store.run_reserved` 的 `except Exception` 抓不到它 ——
    会在 worker 线程里静默逃逸，表现为「job 永远 running」，没有任何报错。
    """


def extract_frames(video: Path, work: Path, fps: float) -> list[Path]:
    """用 ffmpeg 无损抽帧。work 必须是本次运行独有的空目录，避免残留帧污染结果。"""
    work.mkdir(parents=True, exist_ok=True)
    cmd = [FFMPEG, "-y", "-i", str(video)]
    if fps > 0:
        cmd += ["-vf", f"fps={fps}"]
    cmd += ["-start_number", "0", str(work / "s%04d.png")]
    result = run(cmd)
    if result.returncode != 0:
        raise MattingError(f"ffmpeg 抽帧失败:\n{result.stderr[-2000:]}")
    frames = sorted(work.glob("s*.png"))
    if not frames:
        raise MattingError("ffmpeg 没有输出任何帧")
    return frames


def chroma_alpha(rgb: np.ndarray) -> np.ndarray:
    """基于 G - max(R,B) 的软色键，返回背景不透明度 bg_alpha。"""
    r = rgb[:, :, 0].astype(np.float32)
    g = rgb[:, :, 1].astype(np.float32)
    b = rgb[:, :, 2].astype(np.float32)
    excess = g - np.maximum(r, b)
    bg = (excess - KEY_LOW) / (KEY_HIGH - KEY_LOW)
    return np.clip(bg, 0.0, 1.0)


def compute_autocrop(rgbs: list[np.ndarray]) -> tuple[tuple[int, int, int], tuple[int, int, int, int]]:
    """按全部帧的联合前景包围盒取一个正方形画面。

    Seedance 会把方图首帧铺进 16:9 画布（两侧补绿幕），所以直接裁中央方图会切掉
    甩出去的尾巴。这里用"所有帧前景的并集"来定取景框，保证任何一帧都不裁到主体。
    """
    x0f, y0f, x1f, y1f = 10 ** 9, 10 ** 9, -1, -1
    for rgb in rgbs:
        fg = chroma_alpha(rgb) <= 0.5
        ys, xs = np.nonzero(fg)
        if ys.size:
            x0f = min(x0f, int(xs.min()))
            y0f = min(y0f, int(ys.min()))
            x1f = max(x1f, int(xs.max()))
            y1f = max(y1f, int(ys.max()))

    h, w = rgbs[0].shape[:2]

    # 取"能装下并集的最紧正方形"，而不是"最大的正方形"。
    # 差别很实际：1:1 那支若按最大正方形裁就是 960×960 全画布，猫只占 785×800，
    # 剩下一圈透明留白。而运行时是按画布 contain 缩放的——留白越多，
    # 猫在屏幕上就越小。紧裁成 816×816 后，猫的像素尺寸没变，
    # 但显示尺寸从 343px 变成 404px，等于白赚 18%。
    bbox_w = x1f - x0f + 1
    bbox_h = y1f - y0f + 1
    wanted = max(bbox_w, bbox_h) + 2 * CROP_MARGIN
    side = int(min(min(h, w), wanted))

    cx, cy = (x0f + x1f) // 2, (y0f + y1f) // 2
    x0 = max(0, min(cx - side // 2, w - side))
    y0 = max(0, min(cy - side // 2, h - side))
    return (x0, y0, side), (x0f, y0f, x1f, y1f)


def box_contains(box: tuple[int, int, int], union: tuple[int, int, int, int]) -> bool:
    """取景框能不能装下前景并集（**闭区间**：框内最右列是 `x + side - 1`）。

    差 1 就会漏判一整列像素，所以别写成 `x + side >= fx1`。
    """
    x, y, side = box
    fx0, fy0, fx1, fy1 = union
    return x <= fx0 and y <= fy0 and x + side - 1 >= fx1 and y + side - 1 >= fy1


@dataclass(frozen=True)
class CropPlan:
    """一次抠像的取景决策（只算数，不碰像素）。

    `source` 四种：
      - `"auto"`   自己按并集 autocrop（idle 走这条）
      - `"reused"` 动作并集装得进参考框 → 原样复用（触发瞬间零跳位）
      - `"refit"`  装不进 → 退回动作自己的并集取景，再整体缩回 `refit_target`
      - `"off"`    不裁
    """

    box: tuple[int, int, int] | None
    union: tuple[int, int, int, int]
    source: str
    reference: tuple[int, int, int] | None = None
    clipped_by_reference: bool = False
    refit_target: int = 0

    @property
    def refit_upscales(self) -> bool:
        """重取景之后那一步是**放大**吗。

        源画布比参考框还小时会发生（源 480² + 参考框 564 ⇒ ×1.175）：没有新像素，
        只有 LANCZOS 插值 ⇒ 糊。数学上无解（要么裁、要么糊），选糊但必须吼一声。
        """
        return bool(self.box) and self.refit_target > self.box[2]


def plan_crop(
    rgbs: list[np.ndarray],
    *,
    autocrop: bool,
    crop_box: tuple[int, int, int] | None,
) -> CropPlan:
    """决定这次抠像用哪个取景框。

    【动作不再硬复用 idle 的框 2026-09-21】
    「动作必须复用 idle 的框」只在**动作并集装得进那个框**时成立。装不进时它裁掉的是
    真像素 —— 毛球2 的 `grab-release`：并集 `[4,24,637,637]`（634×614），idle 框
    564@(3,31) ⇒ **右溢 71 / 下溢 43**，落地那一帧的脚被整条切掉。裁掉的像素打包侧
    补不回来（`packing` 只能重采样已有的像素），所以改成：

      - 装得进 → 复用参考框（**旧行为逐字节不变**，触发瞬间零跳位）
      - 装不进 → 用动作自己的并集取景（与 idle 同一套 autocrop 规则），再整体缩回
        `refit_target`，于是**一个像素都不裁**

    代价是动作期间形象比 idle 小 `1 - refit_target/box.size`（毛球2 实测 11.6%）。
    这是「少一点」和「缺一块」之间的选择，也比「静默裁切 + 事后才发现」好：
    `clippedByReferenceBox` 会把这件事写明留档，要彻底解决仍需在提示词里约束动作幅度。
    """
    own_box, union = compute_autocrop(rgbs)
    if crop_box is not None:
        cx, cy, side = crop_box
        src_h, src_w = rgbs[0].shape[:2]
        if side <= 0 or cx < 0 or cy < 0 or cx + side > src_w or cy + side > src_h:
            raise MattingError(
                f"crop box ({cx},{cy},{side}) does not fit inside the "
                f"{src_w}x{src_h} source frames"
            )
        # 没有前景（并集是空区间）就无所谓裁不裁，别拿空并集去触发重取景
        empty = union[2] < union[0] or union[3] < union[1]
        if empty or box_contains((cx, cy, side), union):
            return CropPlan((cx, cy, side), union, "reused", (cx, cy, side))
        return CropPlan(own_box, union, "refit", (cx, cy, side), True, side)
    if autocrop:
        return CropPlan(own_box, union, "auto", None, not box_contains(own_box, union))
    return CropPlan(None, union, "off")


def despill(rgb: np.ndarray, alpha: np.ndarray,
            bg_ref: np.ndarray | None = None) -> np.ndarray:
    """把被绿幕污染的颜色反算回真实前景色。

    观测值 = 前景 * alpha + 绿幕 * (1 - alpha)
    => 前景 = (观测 - 绿幕 * (1 - alpha)) / alpha

    bg_ref 必须是视频里实测到的绿幕颜色，不能用理想的 (0,255,0)。
    """
    obs = rgb.astype(np.float32)
    a = alpha[:, :, None]
    bg = GREEN_RGB if bg_ref is None else bg_ref.astype(np.float32)

    # 主路径：反算（在 alpha 足够大时稳定）
    unmixed = (obs - bg * (1.0 - a)) / np.maximum(a, 1e-6)

    # 备用路径：温和去绿，把 G 压到不超过 (R+B)/2
    g = obs[:, :, 1]
    rb_mean = (obs[:, :, 0] + obs[:, :, 2]) * 0.5
    soft = obs.copy()
    soft[:, :, 1] = np.minimum(g, rb_mean)

    use_unmix = (alpha >= UNMIX_MIN_ALPHA)[:, :, None]
    out = np.where(use_unmix, unmixed, soft)
    return np.clip(out, 0.0, 255.0)


def clean_mask(alpha: np.ndarray) -> np.ndarray:
    """形态学清理：去背景噪点、补前景小洞，保持尾巴细结构。"""
    fg = alpha >= 0.5
    if not fg.any():
        return alpha
    labeled, count = ndimage.label(fg)
    if count > 1:
        sizes = ndimage.sum(fg, labeled, range(1, count + 1))
        keep = np.argmax(sizes) + 1
        fg = labeled == keep

    # 填补前景内的小洞（毛色缺口），用 3x3 闭运算
    fg = ndimage.binary_closing(fg, structure=np.ones((3, 3), dtype=bool))
    # 去掉背景里的孤立小点
    fg = ndimage.binary_opening(fg, structure=np.ones((2, 2), dtype=bool))

    # 用清理后的硬 mask 约束 alpha，但保留边缘软过渡
    soft = np.clip(alpha, 0.0, 1.0)
    soft[~fg] *= 0.35          # 背景区域残留压低而非硬切，避免边缘锯齿
    soft[fg] = soft[fg] * 0.65 + 0.35
    return np.clip(soft, 0.0, 1.0)


def compute_color_reference(master_path: Path, mask_alpha: int = 200) -> np.ndarray:
    """从母版提取主体颜色的每通道均值与标准差，作为色彩校正目标。"""
    arr = np.array(Image.open(master_path).convert("RGBA")).astype(np.float32)
    px = arr[:, :, :3][arr[:, :, 3] >= mask_alpha]
    if len(px) < 1000:
        raise MattingError(f"[失败] 母版里找不到足够的不透明主体像素: {master_path}")
    return np.stack([px.mean(axis=0), px.std(axis=0)])  # shape (2, 3)


def match_color(rgb: np.ndarray, alpha: np.ndarray, ref: np.ndarray) -> np.ndarray:
    """把当前帧的主体颜色统计拉回母版（Reinhard 风格的线性色彩迁移）。

    为什么需要：Seedance 生成视频时会重新渲染整个画面。实测毛砌墙那支，
    毛色饱和度从母版的 58 被压到 42（-24%），肉眼表现就是"毛色变浅"。
    母版的颜色才是对齐用户照片的，所以以母版为目标做校正。

    只在不透明主体（alpha>=0.99）上统计，避免半透明边缘的绿残留污染统计量；
    但变换应用到整帧，保证整只猫的颜色一致。

    ⚠️ **线性变换是逐通道各拉各的，会重新制造绿边**（2026-09-15 实测）：
    去绿（`despill`）刚把半透明边缘带压下去，校色又把 G 单独拉高 ——
    需要的 G 增益越大越明显。实测同一支 Mini 视频：
    `edgeGreenFringeRatio` 3e-06 → **0.1298**、`interiorGreenSpillRatio` 0.0 → **0.0134**
    （阈值 0.02 / 0.005）⇒ 四项判据里的「无绿边」直接 FAIL。
    连几乎不用校色的标准档也把 interiorSpill 顶到 0.0071。

    所以这里给一条硬保证：**校色不许把任一像素的绿余量（`G-max(R,B)`）
    抬到它校色前的水平之上**。校色前后都算一遍余量，多出来的部分从 G 上扣掉。
    这不会削掉校色本身（主体该亮的还是亮的），只挡住"被推绿"的那部分。
    """
    out = rgb.astype(np.float32)
    core = alpha >= 0.99
    if int(core.sum()) < 1000:
        return out
    px = out[core]
    src_mean = px.mean(axis=0)
    src_std = px.std(axis=0)
    dst_mean, dst_std = ref[0], ref[1]
    # 标准差过小的通道不做拉伸，避免放大噪声
    gain = np.where(src_std > 1e-3, dst_std / np.maximum(src_std, 1e-3), 1.0)
    corrected = np.clip((out - src_mean) * gain + dst_mean, 0.0, 255.0)

    # 绿余量只许「不增加」：多出来的从 G 扣掉。
    # 扣完仍满足 G >= max(R,B)，所以不会把 G 压到负值。
    before = out[:, :, 1] - np.maximum(out[:, :, 0], out[:, :, 2])
    after = corrected[:, :, 1] - np.maximum(corrected[:, :, 0], corrected[:, :, 2])
    overshoot = np.clip(after - np.maximum(before, 0.0), 0.0, None)
    corrected[:, :, 1] -= overshoot
    return np.clip(corrected, 0.0, 255.0)


def temporal_smooth(stack: np.ndarray, radius: int) -> np.ndarray:
    """沿时间轴做滑动中位数，抑制帧间 alpha 抖动。

    默认关闭（radius=0）。三种选择各有代价，实测结论：

    - 均值：会在运动边缘留下拖影。摇尾巴时某像素 5 帧里只被覆盖 2 帧，
      均值留下 alpha≈0.4 的绿色残影，尾巴像拖了条绿尾巴。实测绿边率 0.11。
    - 中位数：没有拖影（绿边率 0.0001），但运动边缘会以 2-3px 的步长跳进，
      快速甩尾可能看出"台阶感"。
    - 关闭：绿边率 0.0，且实测主体内部 alpha 时间标准差仅 0.0006，
      说明色键本身已经很稳，平滑没有收益，只有副作用。

    所以默认关闭；只有在绿幕不干净、色键明显发抖时才开中位数（不要用均值）。
    """
    if radius <= 0 or stack.shape[0] < 3:
        return stack
    padded = np.pad(stack, ((radius, radius), (0, 0), (0, 0)), mode="edge")
    smoothed = np.empty_like(stack)
    for i in range(stack.shape[0]):
        smoothed[i] = np.median(padded[i:i + 2 * radius + 1], axis=0)
    return smoothed


def estimate_frame_background(rgb: np.ndarray) -> float:
    """估计单帧的绿幕 G 值。

    用画面上下边缘条带里的强绿像素，避开画面中央的主体。
    """
    h = rgb.shape[0]
    band = max(8, h // 12)
    strip = np.concatenate([
        rgb[:band].reshape(-1, 3).astype(np.float32),
        rgb[-band:].reshape(-1, 3).astype(np.float32),
    ])
    excess = strip[:, 1] - np.maximum(strip[:, 0], strip[:, 2])
    strong = strip[excess > 100]
    if not strong.size:
        return float("nan")
    return float(np.median(strong[:, 1]))


def detect_warmup_frames(rgbs: list[np.ndarray], tol: float) -> tuple[int, list[float]]:
    """检测模型"接管"前的过渡帧数量。

    实测发现的坑：Seedance 生成时，开头若干帧还保留着我们输入首帧的绿幕亮度，
    之后模型把背景重画成另一个亮度（毛砌墙那支：首帧 G=250，第 8 帧起稳定在 G=166）。

    这不影响抠像（阈值远低于两者），但会让**首帧的轮廓面积明显偏小**，
    把"面积波动"指标顶高——毛砌墙就因此从 5.04% 变成 6.46%，误判成尾巴不完整。

    做法：逐帧估计背景亮度，从**开头**找第一个进入稳定段的帧。

    【只能从开头找，不能从后往前找】
    2026-08-31 踩坑：首尾帧模式下，模型在结尾还原首帧状态时
    **会把绿幕亮度也一起还原**（实测 G 246→191→248）。
    原实现"从后往前找最后一个偏离帧"会把结尾的回升当成过渡起点，
    判定跳过 120 帧，只剩 1 帧输出——整个抠像废掉。

    而且尾部的绿幕变化本来就不该跳过：模型在那里正是在向锚点姿态收敛，
    那是我们想要的，跳掉反而丢掉了收敛段。
    """
    series = [estimate_frame_background(r) for r in rgbs]
    valid = [v for v in series if np.isfinite(v)]
    if len(valid) < 5:
        return 0, series
    reference = float(np.median(valid))
    # 从前往后找：第一个落入稳定段的帧即为起点
    start = 0
    for i in range(len(series)):
        if np.isfinite(series[i]) and abs(series[i] - reference) <= tol:
            start = i
            break
    return int(min(start, len(rgbs) - 1)), series


def estimate_background(rgbs: list[np.ndarray]) -> np.ndarray:
    """从实际视频估计绿幕参考色。

    不能用理想的 (0,255,0)：H.264 编码后实测背景是 (0,226,1)，
    拿 255 去反算会过度减绿，边缘会偏紫。

    【强绿阈值必须自适应降级 2026-09-03】
    固定 150 只能识别"亮绿幕"：
      - 毛砌墙 G-max(R,B)=221（背景 0,227,6）  -> 150 档命中
      - 建国2  G-max(R,B)=104（背景 41,145,38 深绿）-> 150 档一个像素都没有，
        fallback 成 (0,255,0)，despill 按 255 反算 = 过度减绿，边缘偏紫
    所以从高到低逐级放宽，第一个能采到足够像素的档位即为背景。
    最低档 30 是安全下限：再低会把猫的绿眼睛（G-max(R,B) 约 40~60）当成背景。
    """
    samples = rgbs[::max(1, len(rgbs) // 10)][:10]
    for thr in (150, 100, 60, 30):
        pixels = []
        for rgb in samples:
            r = rgb[:, :, 0].astype(np.float32)
            g = rgb[:, :, 1].astype(np.float32)
            b = rgb[:, :, 2].astype(np.float32)
            strong_green = (g - np.maximum(r, b)) > thr
            if strong_green.sum() > 100:
                pixels.append(rgb[strong_green][:20000])
        if pixels:
            return np.median(np.vstack(pixels), axis=0).astype(np.float32)
    return np.array([0.0, 255.0, 0.0])


def spatial_smooth(alpha: np.ndarray, sigma: float) -> np.ndarray:
    if sigma <= 0:
        return alpha
    return ndimage.gaussian_filter(alpha, sigma=sigma)


def method_not_supported(name: str) -> None:
    raise MattingError(
        f"[未启用] --method {name} 暂未接入。\n"
        f"第一轮先用默认 chroma（绿幕色键）跑通链路。\n"
        f"若 chroma 不通过四项验收，再接入 {name} 并在此文件中实现 {name}_alpha()。"
    )


def rvm_alpha(frames: list[np.ndarray]) -> list[np.ndarray]:
    method_not_supported("rvm")


def sam2_alpha(frames: list[np.ndarray]) -> list[np.ndarray]:
    method_not_supported("sam2")


@dataclass(frozen=True)
class MatteResult:
    out_dir: Path
    frames_dir: Path
    params_path: Path
    frames: tuple[str, ...]
    frame_count: int
    frame_duration_ms: int
    output_size: tuple[int, int]
    source_fps: float
    output_fps: float
    source_size: tuple[int, int]
    warmup_frames_skipped: int
    measured_background_rgb: tuple[float, float, float]
    crop: dict[str, object] | None
    video_sha256: str
    method: str


def matte_video(
    video: Path,
    out_dir: Path,
    *,
    method: str = "chroma",
    fps: float = 0.0,
    size: int = 0,
    temporal: int = 0,
    spatial: float = 0.6,
    frame_duration_ms: int = 0,
    autocrop: bool = True,
    crop_box: tuple[int, int, int] | None = None,
    auto_warmup: bool = True,
    warmup_tol: float = WARMUP_TOL,
    color_match: Path | None = None,
    path_base: Path | None = None,
    log: Callable[[str], None] = print,
) -> MatteResult:
    """跑完整条抠像流程，返回产物位置与参数。

    只产 `frames/f*.png` + `抠像参数.json`，**不写 manifest**：schema 7 包是
    `frames.packing` 的职责。旧脚本会顺手写一份硬编码 `petId: "poc-niuniu-greenscreen"`
    的 schema 6 manifest —— 那是误导，搬进服务时去掉了。
    """
    video = Path(video)
    out_dir = Path(out_dir)
    if method not in {"chroma", "rvm", "sam2"}:
        raise ValueError(f"unsupported matting method: {method}")
    if not video.is_file():
        raise ValueError(f"video does not exist: {video}")
    if size < 0 or temporal < 0 or spatial < 0 or warmup_tol < 0:
        raise ValueError("size / temporal / spatial / warmup_tol must be non-negative")
    # 校色母版在校验阶段就查存在性：抽完 289 帧才发现文件不在，白等一分钟
    master_path = None
    if color_match is not None:
        master_path = Path(color_match).resolve()
        if not master_path.is_file():
            raise ValueError(f"color-match master does not exist: {master_path}")

    src_fps = probe_fps(video)
    effective_fps = fps if fps > 0 else src_fps
    src_w, src_h = probe_size(video)
    log(f"[输入] {video.name}  源 fps={src_fps:.3f} -> 抽帧 fps={effective_fps:.3f}  "
        f"源尺寸={src_w}x{src_h}")

    # 原始帧落在系统临时目录：既不污染产物目录，也不需要删项目文件
    work = Path(tempfile.mkdtemp(prefix="poc_matting_raw_"))
    raw_frames = extract_frames(video, work, effective_fps)
    log(f"[抽帧] {len(raw_frames)} 帧  (临时目录 {work})")

    # 读全部帧到内存（288 帧 @640，约 340MB，可接受）
    rgbs = [np.array(Image.open(path).convert("RGB")) for path in raw_frames]

    # 过渡帧：开头若干帧可能还保留输入首帧的绿幕亮度，之后模型才「接管」。
    # 这些帧的轮廓面积会明显偏小，把面积波动指标顶高，需要跳过。
    warmup = 0
    bg_series: list[float] = []
    if auto_warmup:
        warmup, bg_series = detect_warmup_frames(rgbs, warmup_tol)
        if warmup > 0:
            stable = [value for value in bg_series[warmup:] if np.isfinite(value)]
            log(f"[过渡] 检测到 {warmup} 帧过渡帧，背景 G 从 "
                f"{bg_series[0]:.0f} 变为 {np.median(stable):.0f}（稳定段极差 "
                f"{max(stable) - min(stable):.1f}）-> 跳过")
            rgbs = rgbs[warmup:]

    # 取景：Seedance 常把方图首帧铺进 16:9 画布，需要按「全部帧前景并集」裁正方形，
    # 否则会切掉甩出原方图范围的尾巴。
    #
    # `crop_box` 给了就**优先用它**（跳过 autocrop）：**动作复用 idle 的框**，
    # 两支视频各自 autocrop 会让触发瞬间错位（05 的 yawn 就是这么错位 27px 的）。
    # 但**装不下时不再硬复用** —— 那会裁掉真像素，详见 `plan_crop`。
    crop_record = None
    plan = plan_crop(rgbs, autocrop=autocrop, crop_box=crop_box)
    if plan.box is not None:
        x, y, side = plan.box
        rgbs = [r[y:y + side, x:x + side] for r in rgbs]
        crop_record = {
            "x": x, "y": y, "size": side,
            "source": plan.source,
            "unionForegroundBBox": list(plan.union),
            "clipsForeground": not box_contains(plan.box, plan.union),
        }
        if plan.reference is not None:
            crop_record["referenceBox"] = list(plan.reference)
            crop_record["clippedByReferenceBox"] = plan.clipped_by_reference
        scale_note = ""
        if plan.refit_target and plan.refit_target != side:
            target = plan.refit_target
            rgbs = [
                np.array(Image.fromarray(r).resize((target, target), Image.LANCZOS))
                for r in rgbs
            ]
            upscale = target / side
            crop_record["refitScale"] = round(upscale, 4)
            scale_note = f"，再整体缩回 {target}x{target}（×{upscale:.3f}）"
            # 源画布比参考框还小时，这一步是**放大**：没有新像素，只会把 LANCZOS 的
            # 插值糊上去（源 480² 配 564 参考框 ⇒ ×1.175）。数学上无解 —— 要么裁、要么糊。
            # 选了糊（不丢内容），但必须让人看见：这是**不可逆**的画质损失。
            if plan.refit_upscales:
                crop_record["refitUpscale"] = True
                log(f"[警告] 动作并集需要 {side}px，而源视频画布只有那么大："
                    f"缩回参考框是**放大** ×{upscale:.3f} —— 细节是插值出来的，糊了就回不来。"
                    "处置：让 Seedance 直接输出 ≥ 参考框的方图，或收紧动作幅度。")
        if plan.source == "reused":
            log(f"[取景] 复用调用方给的 crop {side}x{side} @({x},{y})"
                f"（与 idle 同一个框，触发时不跳位）")
        elif plan.source == "refit":
            rx, ry, rsize = plan.reference or (0, 0, 0)
            log(f"[取景] 前景 bbox={list(plan.union)} 装不进参考框 {rsize}@({rx},{ry})："
                "硬复用会裁掉真像素，而裁掉的像素打包侧补不回来。")
            log(f"[取景] 改按动作自身并集取景 {side}x{side} @({x},{y}){scale_note} —— 不裁像素；"
                "代价是动作期间形象比 idle 略小，彻底解决仍要在提示词里约束动作幅度。")
        else:
            log(f"[取景] 联合前景 bbox={list(plan.union)} -> 裁 {side}x{side} @({x},{y})")
        if crop_record["clipsForeground"]:
            log("[警告] 取景框装不下全部前景，主体会被裁切！"
                "请关掉 autocrop 手动指定，或让 Seedance 输出与首帧同比例")

    if size > 0 and rgbs[0].shape[0] != size:
        resized = [Image.fromarray(r).resize((size, size), Image.LANCZOS) for r in rgbs]
        rgbs = [np.array(image) for image in resized]
        log(f"[缩放] -> {size}x{size}")

    # 实测绿幕色：H.264 之后不是理想的 (0,255,0)，拿理想值反算会过度减绿
    bg_ref = estimate_background(rgbs)
    log(f"[绿幕] 实测参考色 RGB=({bg_ref[0]:.1f}, {bg_ref[1]:.1f}, {bg_ref[2]:.1f})"
        f"   （脚本默认值 (0,255,0)）")

    color_ref = None
    if master_path is not None:
        color_ref = compute_color_reference(master_path)
        log(f"[校色] 目标母版 {master_path.name}  "
            f"主体 RGB=({color_ref[0][0]:.0f},{color_ref[0][1]:.0f},{color_ref[0][2]:.0f})  "
            f"饱和≈{(color_ref[0].max() - color_ref[0].min()):.0f}")

    if method == "rvm":
        alphas = rvm_alpha(rgbs)
    elif method == "sam2":
        alphas = sam2_alpha(rgbs)
    else:
        alphas = [chroma_alpha(r) for r in rgbs]

    stack = np.stack(alphas).astype(np.float32)
    fg_alpha = 1.0 - stack

    if spatial > 0:
        fg_alpha = np.stack([spatial_smooth(a, spatial) for a in fg_alpha])
    fg_alpha = np.stack([clean_mask(a) for a in fg_alpha])
    if temporal > 0:
        fg_alpha = temporal_smooth(fg_alpha, temporal)
    fg_alpha = np.clip(fg_alpha, 0.0, 1.0)

    frames_dir = out_dir / "frames"
    frames_dir.mkdir(parents=True, exist_ok=True)
    stale = sorted(frames_dir.glob("f*.png"))[len(rgbs):]
    if stale:
        log(f"[警告] 目录里存在 {len(stale)} 个上一轮残留帧（{stale[0].name} 起），"
            f"本次不引用它们，请手动删除以免混淆")

    duration_ms = (
        frame_duration_ms if frame_duration_ms > 0 else int(round(1000.0 / effective_fps))
    )
    frame_rels: list[str] = []
    for index, (rgb, alpha) in enumerate(zip(rgbs, fg_alpha)):
        clean_rgb = despill(rgb, alpha, bg_ref)
        if color_ref is not None:
            clean_rgb = match_color(clean_rgb, alpha, color_ref)
        composed = np.dstack([clean_rgb, alpha * 255.0]).astype(np.uint8)
        relative = f"frames/f{index:03d}.png"
        Image.fromarray(composed, mode="RGBA").save(out_dir / relative)
        frame_rels.append(relative)

    # 首帧另存静态底图（旧流程的下游按这个名字找，保留以免打断）
    shutil.copy2(out_dir / frame_rels[0], out_dir / "body.png")

    output_size = (int(rgbs[0].shape[1]), int(rgbs[0].shape[0]))
    params = {
        "video": rel_to(video, path_base),
        "videoSha256": sha256_of(video),
        "method": method,
        "sourceFps": round(src_fps, 3),
        "outputFps": round(effective_fps, 3),
        "frameCount": len(frame_rels),
        "frameDurationMs": duration_ms,
        "outputSize": list(output_size),
        "keyLow": KEY_LOW,
        "keyHigh": KEY_HIGH,
        "measuredBackgroundRGB": [round(float(value), 2) for value in bg_ref],
        "temporalFilter": "median",
        "temporalRadius": temporal,
        "spatialSigma": spatial,
        "unmixMinAlpha": UNMIX_MIN_ALPHA,
        "sourceSize": [src_w, src_h],
        "crop": crop_record,
        "warmupFramesSkipped": warmup,
        "backgroundGSeriesHead": [
            round(float(value), 1) for value in bg_series[:12] if np.isfinite(value)
        ],
    }
    params_path = out_dir / "抠像参数.json"
    params_path.write_text(
        json.dumps(params, ensure_ascii=False, indent=2), encoding="utf-8"
    )

    total_kb = sum(path.stat().st_size for path in frames_dir.glob("*.png")) // 1024
    log(f"[输出] {len(frame_rels)} 帧 -> {frames_dir}  ({total_kb} KB)")
    log(f"[参数] method={method} fps={effective_fps:.2f} 帧时长={duration_ms}ms "
        f"时间平滑={temporal} 空间平滑={spatial}")

    return MatteResult(
        out_dir=out_dir,
        frames_dir=frames_dir,
        params_path=params_path,
        frames=tuple(frame_rels),
        frame_count=len(frame_rels),
        frame_duration_ms=duration_ms,
        output_size=output_size,
        source_fps=src_fps,
        output_fps=effective_fps,
        source_size=(src_w, src_h),
        warmup_frames_skipped=warmup,
        measured_background_rgb=tuple(float(value) for value in bg_ref),
        crop=crop_record,
        video_sha256=params["videoSha256"],
        method=method,
    )
