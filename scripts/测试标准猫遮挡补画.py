from __future__ import annotations

import json
import sys
from pathlib import Path

import cv2
import numpy as np
from PIL import Image


ROOT = Path(r"D:\PetBabyAssets\cat-a-live2d-v1\标准猫")
LAYERS = ROOT / "02-分层源"
CONTRACT = LAYERS / "图层合同.json"
WORK = LAYERS / "工作稿"


def load_rgba(path: Path) -> np.ndarray:
    return np.asarray(Image.open(path).convert("RGBA"))


def fill_small_enclosed_alpha_holes(image: np.ndarray, maximum_area: int = 64) -> np.ndarray:
    repaired = image.copy()
    transparent = repaired[:, :, 3] < 16
    flood = np.zeros(transparent.shape, dtype=np.uint8)
    flood[0, :] = transparent[0, :]
    flood[-1, :] = transparent[-1, :]
    flood[:, 0] = transparent[:, 0]
    flood[:, -1] = transparent[:, -1]
    previous = np.zeros_like(flood)
    kernel = cv2.getStructuringElement(cv2.MORPH_CROSS, (3, 3))
    while not np.array_equal(previous, flood):
        previous = flood.copy()
        flood = cv2.dilate(flood, kernel)
        flood &= transparent.astype(np.uint8)
    enclosed = transparent & ~(flood > 0)
    count, labels, stats, _ = cv2.connectedComponentsWithStats(
        enclosed.astype(np.uint8), 4
    )
    repair_mask = np.zeros(transparent.shape, dtype=np.uint8)
    for index in range(1, count):
        if int(stats[index, cv2.CC_STAT_AREA]) <= maximum_area:
            repair_mask[labels == index] = 255
    if np.any(repair_mask):
        repaired[:, :, :3] = cv2.inpaint(
            repaired[:, :, :3], repair_mask, 3, cv2.INPAINT_TELEA
        )
        repaired[:, :, 3][repair_mask > 0] = 255
    return repaired


def alpha_count(image: np.ndarray) -> int:
    return int(np.count_nonzero(image[:, :, 3]))


def black_opaque_ratio(image: np.ndarray) -> float:
    visible = image[:, :, 3] > 0
    if not np.any(visible):
        return 1.0
    rgb = image[:, :, :3]
    black = np.all(rgb < 12, axis=2) & visible
    return float(np.count_nonzero(black) / np.count_nonzero(visible))


def has_black_background_leak(image: np.ndarray) -> bool:
    visible = image[:, :, 3] > 0
    black = np.all(image[:, :, :3] < 12, axis=2) & visible
    count, _, stats, _ = cv2.connectedComponentsWithStats(black.astype(np.uint8), 8)
    height, width = black.shape
    for index in range(1, count):
        x, y, component_width, component_height, area = map(int, stats[index])
        touches_edge = (
            x == 0
            or y == 0
            or x + component_width == width
            or y + component_height == height
        )
        if touches_edge or area >= 5_000:
            return True
    return False


def texture_std(image: np.ndarray) -> float:
    visible = image[:, :, 3] > 0
    if not np.any(visible):
        return 0.0
    gray = cv2.cvtColor(image[:, :, :3], cv2.COLOR_RGB2GRAY)
    laplacian = cv2.Laplacian(gray, cv2.CV_32F)
    return float(laplacian[visible].std())


def assert_reserve(path: Path, minimum_pixels: int, minimum_texture: float) -> None:
    image = load_rgba(path)
    assert alpha_count(image) >= minimum_pixels, f"{path.name} 仍为空或覆盖不足"
    assert not has_black_background_leak(image), f"{path.name} 含不透明黑色背景泄漏"
    assert texture_std(image) >= minimum_texture, f"{path.name} 纹理过度平滑或拉伸"


def assert_tail_reserve_follows_tail() -> None:
    tail = load_rgba(WORK / "17-tail.png")[:, :, 3] > 0
    body_under_tail = load_rgba(WORK / "01-bodyUnderTail.png")[:, :, 3] > 0
    tail_reserve = load_rgba(WORK / "18-tailReserve.png")[:, :, 3] > 0
    kernel = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (145, 145))
    curved_envelope = cv2.dilate(tail.astype(np.uint8), kernel) > 0
    for name, reserve in (
        ("bodyUnderTail", body_under_tail),
        ("tailReserve", tail_reserve),
    ):
        outside = int(np.count_nonzero(reserve & ~curved_envelope))
        total = int(np.count_nonzero(reserve))
        assert outside / total <= 0.01, f"{name} 未沿尾巴曲线生成，存在矩形补画边界"
        overlap = int(np.count_nonzero(reserve & tail))
        assert overlap >= 12_000, f"{name} 未覆盖尾根到尾身的真实遮挡区"


def assert_occlusion_reserve_is_local() -> None:
    reserve = load_rgba(WORK / "19-occlusionReserve.png")[:, :, 3] > 0
    moving_parts = np.zeros(reserve.shape, dtype=np.uint8)
    for name in (
        "04-earLeft.png", "05-earRight.png",
        "06-eyeWhiteLeft.png", "07-eyeWhiteRight.png",
        "08-irisLeft.png", "09-irisRight.png",
        "10-upperLidLeft.png", "11-upperLidRight.png",
        "12-lowerLidLeft.png", "13-lowerLidRight.png",
        "14-muzzle.png", "15-frontLegLeft.png", "16-frontLegRight.png",
    ):
        moving_parts = np.maximum(
            moving_parts,
            (load_rgba(WORK / name)[:, :, 3] > 0).astype(np.uint8),
        )
    allowed = moving_parts > 0
    outside = int(np.count_nonzero(reserve & ~allowed))
    total = int(np.count_nonzero(reserve))
    assert total > 0, "occlusionReserve 为空"
    assert outside / total <= 0.01, (
        "occlusionReserve 覆盖了活动件原始洞口以外的正常身体，会在逐层隐藏时形成接缝: "
        f"outside={outside / total:.3f}"
    )


def assert_ears_do_not_overlap_eyes() -> None:
    ears = np.maximum(
        load_rgba(WORK / "04-earLeft.png")[:, :, 3],
        load_rgba(WORK / "05-earRight.png")[:, :, 3],
    ) > 0
    eyes = np.zeros(ears.shape, dtype=np.uint8)
    for name in (
        "06-eyeWhiteLeft.png", "07-eyeWhiteRight.png",
        "08-irisLeft.png", "09-irisRight.png",
        "10-upperLidLeft.png", "11-upperLidRight.png",
        "12-lowerLidLeft.png", "13-lowerLidRight.png",
    ):
        eyes = np.maximum(eyes, load_rgba(WORK / name)[:, :, 3])
    overlap = int(np.count_nonzero(ears & (eyes > 0)))
    assert overlap == 0, f"耳朵层污染眼睛活动区: overlap={overlap}"


def assert_reserve_boundary_matches_master(master: np.ndarray) -> None:
    for filename in ("19-occlusionReserve.png", "01-bodyUnderTail.png"):
        reserve = load_rgba(WORK / filename)
        alpha = reserve[:, :, 3] > 0
        if filename == "19-occlusionReserve.png":
            # 无耳头盖与无肢身体的边缘是活动件移除后新出现的合法外轮廓；
            # 这里只检查眼、眼睑和口鼻洞口与原主体相接的内部接缝。
            internal_holes = np.zeros(alpha.shape, dtype=np.uint8)
            for name in (
                "06-eyeWhiteLeft.png", "07-eyeWhiteRight.png",
                "08-irisLeft.png", "09-irisRight.png",
                "10-upperLidLeft.png", "11-upperLidRight.png",
                "12-lowerLidLeft.png", "13-lowerLidRight.png",
                "14-muzzle.png",
            ):
                internal_holes = np.maximum(
                    internal_holes,
                    (load_rgba(WORK / name)[:, :, 3] > 0).astype(np.uint8),
                )
            alpha &= internal_holes > 0
        eroded = cv2.erode(alpha.astype(np.uint8), np.ones((5, 5), np.uint8)) > 0
        boundary = alpha & ~eroded & (master[:, :, 3] > 0)
        delta = np.mean(
            np.abs(
                reserve[:, :, :3].astype(np.float32)
                - master[:, :, :3].astype(np.float32)
            ),
            axis=2,
        )
        values = delta[boundary]
        assert values.size >= 1_000, f"{filename} 没有可验证的内部补画边界"
        assert np.percentile(values, 95) <= 8.0, (
            f"{filename} 与母版洞口边缘颜色不连续，会形成硬接缝: "
            f"p95={np.percentile(values, 95):.2f}"
        )


def assert_hidden_tail_is_actually_absent(master: np.ndarray) -> None:
    evidence = load_rgba(WORK / "验收证据" / "07-隐藏尾巴.png")
    tail = load_rgba(WORK / "17-tail.png")[:, :, 3] > 0
    body_under_tail = load_rgba(WORK / "01-bodyUnderTail.png")[:, :, 3] > 0
    _, reserve_x = np.where(body_under_tail)
    right_edge = int(reserve_x.max())
    x_grid = np.broadcast_to(np.arange(tail.shape[1]), tail.shape)
    free_tail = tail & (x_grid > right_edge + 16)
    assert np.count_nonzero(free_tail) >= 40_000, "尾巴自由段识别失败"
    remaining = int(np.count_nonzero((evidence[:, :, 3] > 0) & free_tail))
    assert remaining == 0, (
        "隐藏尾巴证据仍显示外伸尾段，补画只能覆盖尾根后的身体"
    )
    changed = np.any(evidence[:, :, :3] != master[:, :, :3], axis=2)
    assert np.count_nonzero(changed & body_under_tail) >= 1_000, "尾根后的身体补画未生效"


def assert_hidden_ears_have_smooth_head_cap() -> None:
    image = load_rgba(WORK / "验收证据" / "04-隐藏双耳.png")
    alpha = image[:, :, 3] > 0
    top_y = []
    for x in range(780, 1261):
        rows = np.flatnonzero(alpha[:, x])
        if rows.size:
            top_y.append(int(rows[0]))
    top_y_array = np.asarray(top_y)
    assert top_y_array.size >= 400, "无耳头盖轮廓覆盖不足"
    second_difference = np.abs(np.diff(top_y_array, n=2))
    assert np.percentile(second_difference, 99) <= 8, "无耳头盖轮廓存在密集锯齿"
    assert second_difference.max() <= 24, "无耳头盖轮廓存在几何硬折点"


def assert_neutral_has_no_enclosed_transparent_holes() -> None:
    image = load_rgba(WORK / "验收证据" / "01-中性重组.png")
    transparent = image[:, :, 3] < 16
    flood = np.zeros(transparent.shape, dtype=np.uint8)
    flood[0, :] = transparent[0, :]
    flood[-1, :] = transparent[-1, :]
    flood[:, 0] = transparent[:, 0]
    flood[:, -1] = transparent[:, -1]
    previous = np.zeros_like(flood)
    kernel = cv2.getStructuringElement(cv2.MORPH_CROSS, (3, 3))
    while not np.array_equal(previous, flood):
        previous = flood.copy()
        flood = cv2.dilate(flood, kernel)
        flood &= transparent.astype(np.uint8)
    enclosed = transparent & ~(flood > 0)
    component_count, _, stats, _ = cv2.connectedComponentsWithStats(
        enclosed.astype(np.uint8), 4
    )
    holes = []
    for index in range(1, component_count):
        x, y, width, height, area = map(int, stats[index])
        if area >= 4:
            holes.append(f"{area}px@{x},{y}..{x + width - 1},{y + height - 1}")
    assert not holes, f"中性重组含封闭透明穿孔: {', '.join(holes)}"


def assert_hidden_anatomy_is_replaced(master: np.ndarray, underpainting: np.ndarray) -> None:
    detachable = np.zeros(master.shape[:2], dtype=np.uint8)
    for name in (
        "04-earLeft.png", "05-earRight.png",
        "06-eyeWhiteLeft.png", "07-eyeWhiteRight.png",
        "08-irisLeft.png", "09-irisRight.png",
        "10-upperLidLeft.png", "11-upperLidRight.png",
        "12-lowerLidLeft.png", "13-lowerLidRight.png",
        "14-muzzle.png", "15-frontLegLeft.png", "16-frontLegRight.png",
        "17-tail.png",
    ):
        detachable = np.maximum(detachable, load_rgba(WORK / name)[:, :, 3])
    color_delta = np.mean(
        np.abs(
            underpainting[:, :, :3].astype(np.float32)
            - master[:, :, :3].astype(np.float32)
        ),
        axis=2,
    )
    detachable_pixels = detachable > 127
    replaced_ratio = float(np.mean(color_delta[detachable_pixels] >= 8.0))
    assert replaced_ratio >= 0.60, (
        f"隐藏区域仍残留原活动部件颜色: replaced={replaced_ratio:.3f}"
    )


def main() -> int:
    contract = json.loads(CONTRACT.read_text(encoding="utf-8"))
    assert contract["contractVersion"] == "LayerContractV1"
    underpainting = load_rgba(WORK / "标准猫-A-v1-完整底图.png")
    assert underpainting.shape == (2048, 2048, 4), "完整底图必须保持 2048×2048 RGBA"
    assert alpha_count(underpainting) >= 350_000, "完整底图主体覆盖不足"
    assert np.count_nonzero(underpainting[[0, -1], :, 3]) == 0, "完整底图上下边缘不透明"
    assert np.count_nonzero(underpainting[:, [0, -1], 3]) == 0, "完整底图左右边缘不透明"

    assert_reserve(WORK / "01-bodyUnderTail.png", minimum_pixels=20_000, minimum_texture=4.0)
    assert_reserve(WORK / "18-tailReserve.png", minimum_pixels=12_000, minimum_texture=4.0)
    assert_reserve(WORK / "19-occlusionReserve.png", minimum_pixels=120_000, minimum_texture=4.0)
    assert_tail_reserve_follows_tail()
    assert_occlusion_reserve_is_local()
    assert_ears_do_not_overlap_eyes()

    neutral_difference = load_rgba(WORK / "验收证据" / "02-中性重组差异.png")
    assert np.count_nonzero(neutral_difference) == 0, "中性重组与批准母版不一致"
    assert_neutral_has_no_enclosed_transparent_holes()

    master = fill_small_enclosed_alpha_holes(
        load_rgba(ROOT / "01-母版" / "标准猫-A-v1.png")
    )
    assert_reserve_boundary_matches_master(master)
    assert_hidden_anatomy_is_replaced(master, underpainting)
    assert_hidden_tail_is_actually_absent(master)
    assert_hidden_ears_have_smooth_head_cap()
    master_alpha = master[:, :, 3] > 0
    for index in range(3, 8):
        evidence = next((WORK / "验收证据").glob(f"{index:02d}-*.png"))
        evidence_alpha = load_rgba(evidence)[:, :, 3] > 0
        assert not np.any(evidence_alpha & ~master_alpha), (
            f"{evidence.name} 的补画超出批准母版轮廓"
        )

    import zipfile

    with zipfile.ZipFile(WORK / "标准猫-A-v1.ora") as archive:
        stack = archive.read("stack.xml").decode("utf-8")
    for name in (
        "body", "bodyUnderTail", "chest", "head", "earLeft", "earRight",
        "eyeWhiteLeft", "eyeWhiteRight", "irisLeft", "irisRight",
        "upperLidLeft", "upperLidRight", "lowerLidLeft", "lowerLidRight",
        "muzzle", "frontLegLeft", "frontLegRight", "tail", "tailReserve",
        "occlusionReserve",
    ):
        assert f'name="{name}"' in stack, f"ORA 缺少图层: {name}"
    print("标准猫遮挡补画像素门槛通过")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AssertionError as error:
        print(f"标准猫遮挡补画像素门槛失败: {error}", file=sys.stderr)
        raise SystemExit(1)
