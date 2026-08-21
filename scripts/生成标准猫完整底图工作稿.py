from __future__ import annotations

from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile
import xml.etree.ElementTree as ET

import cv2
import numpy as np
from PIL import Image, ImageChops, ImageDraw
from scipy.ndimage import distance_transform_edt


ROOT = Path(r"D:\PetBabyAssets\cat-a-live2d-v1\标准猫")
MASTER = ROOT / "01-母版" / "标准猫-A-v1.png"
LAYERS = ROOT / "02-分层源"
WORK = LAYERS / "工作稿"
EVIDENCE = WORK / "验收证据"
REFERENCE = WORK / "标准猫-A-v1-补画参考.png"


def shape_mask(size, *, ellipses=(), polygons=(), rectangles=()):
    mask = Image.new("L", size, 0)
    draw = ImageDraw.Draw(mask)
    for box in ellipses:
        draw.ellipse(box, fill=255)
    for points in polygons:
        draw.polygon(points, fill=255)
    for box in rectangles:
        draw.rectangle(box, fill=255)
    return mask


def union(*masks):
    result = Image.new("L", masks[0].size, 0)
    for mask in masks:
        result = ImageChops.lighter(result, mask)
    return result


def intersect(*masks):
    result = masks[0]
    for mask in masks[1:]:
        result = ImageChops.multiply(result, mask)
    return result


def subtract(base, *holes):
    result = base
    for hole in holes:
        result = ImageChops.subtract(result, hole)
    return result


def dilate(mask: Image.Image, pixels: int) -> Image.Image:
    source = np.asarray(mask)
    kernel = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (pixels * 2 + 1, pixels * 2 + 1))
    return Image.fromarray(cv2.dilate(source, kernel), "L")


def feather_overlap(mask: Image.Image, padding: int = 16, sigma: float = 6.0) -> Image.Image:
    source = np.asarray(mask)
    expanded = np.asarray(dilate(mask, padding))
    blurred = cv2.GaussianBlur(expanded, (0, 0), sigma)
    return Image.fromarray(np.maximum(source, blurred), "L")


def extract_subject(reference_rgb: np.ndarray) -> np.ndarray:
    hsv = cv2.cvtColor(reference_rgb, cv2.COLOR_RGB2HSV)
    mask = np.where((hsv[:, :, 1] > 35) & (hsv[:, :, 2] > 25), 255, 0).astype(np.uint8)
    mask = cv2.morphologyEx(mask, cv2.MORPH_CLOSE, np.ones((11, 11), np.uint8))
    count, labels, stats, _ = cv2.connectedComponentsWithStats(mask, 8)
    if count < 2:
        raise RuntimeError("完整底图候选没有可识别主体")
    label = 1 + int(np.argmax(stats[1:, cv2.CC_STAT_AREA]))
    component = np.where(labels == label, 255, 0).astype(np.uint8)
    contours, _ = cv2.findContours(component, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
    filled = np.zeros_like(component)
    cv2.drawContours(filled, contours, -1, 255, -1)
    return filled


def extend_subject_colors(rgb: np.ndarray, subject: np.ndarray) -> np.ndarray:
    filled = subject > 0
    # imagegen 预览可能把棋盘格烘焙进 RGB；主体外颜色绝不能参与
    # reserve 扩边，否则会在前爪底部和尾根形成白色描边。
    # 最近邻二维传播保留真实边缘毛色，避免逐通道最大值膨胀产生竖条。
    reliable = cv2.erode(
        filled.astype(np.uint8),
        cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (25, 25)),
    ).astype(bool)
    if not np.any(reliable):
        raise RuntimeError("完整底图候选没有可靠的毛发颜色核心")
    _, nearest = distance_transform_edt(~reliable, return_indices=True)
    output = rgb.copy()
    propagated = rgb[nearest[0], nearest[1]]
    output[~reliable] = propagated[~reliable]
    return output


def smooth_subject_alpha(subject: np.ndarray) -> np.ndarray:
    binary = np.where(subject >= 96, 255, 0).astype(np.uint8)
    kernel = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (25, 25))
    binary = cv2.morphologyEx(binary, cv2.MORPH_CLOSE, kernel)
    count, labels, stats, _ = cv2.connectedComponentsWithStats(binary, 8)
    if count < 2:
        raise RuntimeError("完整底图候选平滑后没有主体")
    label = 1 + int(np.argmax(stats[1:, cv2.CC_STAT_AREA]))
    largest = np.where(labels == label, 255, 0).astype(np.uint8)
    return cv2.GaussianBlur(largest, (0, 0), 1.2)


def align_underpainting(master: Image.Image) -> tuple[np.ndarray, np.ndarray, int, float]:
    master_rgba = np.asarray(master)
    reference_original = np.asarray(Image.open(REFERENCE).convert("RGB"))
    subject = extract_subject(reference_original)
    reference_extended = extend_subject_colors(reference_original, subject)

    sift = cv2.SIFT_create(nfeatures=8000, contrastThreshold=0.015)
    ref_points, ref_desc = sift.detectAndCompute(
        cv2.cvtColor(reference_original, cv2.COLOR_RGB2GRAY), subject
    )
    master_points, master_desc = sift.detectAndCompute(
        cv2.cvtColor(master_rgba[:, :, :3], cv2.COLOR_RGB2GRAY), master_rgba[:, :, 3]
    )
    pairs = cv2.BFMatcher().knnMatch(ref_desc, master_desc, k=2)
    good = [first for first, second in pairs if first.distance < 0.76 * second.distance]
    if len(good) < 40:
        raise RuntimeError(f"完整底图候选配准匹配不足: {len(good)}")
    source = np.float32([ref_points[item.queryIdx].pt for item in good])
    target = np.float32([master_points[item.trainIdx].pt for item in good])
    matrix, inliers = cv2.estimateAffinePartial2D(
        source,
        target,
        method=cv2.RANSAC,
        ransacReprojThreshold=6,
        maxIters=10000,
        confidence=0.999,
    )
    if matrix is None or inliers is None:
        raise RuntimeError("完整底图候选无法配准")
    accepted = inliers.ravel().astype(bool)
    predicted = cv2.transform(source[None, :, :], matrix)[0]
    median_error = float(np.median(np.linalg.norm(predicted[accepted] - target[accepted], axis=1)))
    inlier_count = int(np.count_nonzero(accepted))
    if inlier_count < 40 or median_error > 2.5:
        raise RuntimeError(
            f"完整底图候选配准质量不足: inliers={inlier_count}, median={median_error:.3f}"
        )

    size = master.size
    warped_rgb = cv2.warpAffine(
        reference_extended,
        matrix,
        size,
        flags=cv2.INTER_LANCZOS4,
        borderMode=cv2.BORDER_CONSTANT,
    )
    warped_subject = cv2.warpAffine(
        subject,
        matrix,
        size,
        flags=cv2.INTER_CUBIC,
        borderMode=cv2.BORDER_CONSTANT,
    )
    warped_subject = smooth_subject_alpha(warped_subject)
    return warped_rgb, warped_subject, inlier_count, median_error


def cut(source: Image.Image, alpha: Image.Image) -> Image.Image:
    output = source.copy()
    output.putalpha(alpha)
    return output


def candidate_cut(rgb: np.ndarray, alpha: Image.Image) -> Image.Image:
    rgba = np.zeros((rgb.shape[0], rgb.shape[1], 4), dtype=np.uint8)
    rgba[:, :, :3] = rgb
    rgba[:, :, 3] = np.asarray(alpha)
    return Image.fromarray(rgba, "RGBA")


def blend_inside_holes(
    master_rgb: np.ndarray,
    candidate_rgb: np.ndarray,
    hole_mask: Image.Image,
    transition: int = 28,
) -> np.ndarray:
    """让补画在洞口边缘继承母版颜色，向内部平滑过渡到真实底图。"""
    holes = np.asarray(hole_mask) > 0
    if not np.any(holes):
        raise RuntimeError("遮挡补画没有有效洞口")
    distance = cv2.distanceTransform(holes.astype(np.uint8), cv2.DIST_L2, 5)
    # 最外侧 4px 完全继承母版，给抗锯齿和纹理边缘留出安全带；
    # 从安全带内侧才开始向真实补画过渡。
    weight = np.clip((distance - 4.0) / transition, 0.0, 1.0)
    weight = weight[:, :, None]
    blended = (
        master_rgb.astype(np.float32) * (1.0 - weight)
        + candidate_rgb.astype(np.float32) * weight
    )
    return np.clip(blended, 0, 255).astype(np.uint8)


def compose(layer_files: list[str], output_path: Path) -> Image.Image:
    output = Image.new("RGBA", (2048, 2048))
    for filename in layer_files:
        output.alpha_composite(Image.open(WORK / filename).convert("RGBA"))
    output.save(output_path)
    return output


def fill_small_enclosed_alpha_holes(image: Image.Image, maximum_area: int = 64) -> Image.Image:
    """填平主体内部由抠图噪声形成的小型透明孔，不改变外轮廓。"""
    rgba = np.asarray(image.convert("RGBA")).copy()
    transparent = rgba[:, :, 3] < 16
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
        area = int(stats[index, cv2.CC_STAT_AREA])
        if area <= maximum_area:
            repair_mask[labels == index] = 255
    if np.any(repair_mask):
        rgba[:, :, :3] = cv2.inpaint(
            rgba[:, :, :3], repair_mask, 3, cv2.INPAINT_TELEA
        )
        rgba[:, :, 3][repair_mask > 0] = 255
    return Image.fromarray(rgba, "RGBA")


def write_ora(files: list[tuple[str, str, bool]], merged: Path) -> None:
    image = ET.Element("image", {"w": "2048", "h": "2048", "name": "标准猫-A-v1"})
    stack = ET.SubElement(image, "stack", {"name": "标准猫-A-v1"})
    with ZipFile(WORK / "标准猫-A-v1.ora", "w", ZIP_DEFLATED) as archive:
        archive.writestr("mimetype", "image/openraster", compress_type=0)
        for index, (filename, layer_name, visible) in enumerate(reversed(files)):
            source = f"data/{index:02d}.png"
            ET.SubElement(
                stack,
                "layer",
                {
                    "name": layer_name,
                    "src": source,
                    "visibility": "visible" if visible else "hidden",
                },
            )
            archive.write(WORK / filename, source)
        archive.writestr(
            "stack.xml", ET.tostring(image, encoding="utf-8", xml_declaration=True)
        )
        archive.write(merged, "mergedimage.png")


def main() -> None:
    WORK.mkdir(exist_ok=True)
    EVIDENCE.mkdir(exist_ok=True)
    master = fill_small_enclosed_alpha_holes(Image.open(MASTER).convert("RGBA"))
    size = master.size
    cat = master.getchannel("A")
    warped_rgb, warped_subject_array, inliers, median_error = align_underpainting(master)
    warped_subject = Image.fromarray(warped_subject_array, "L")
    ear_left = intersect(cat, shape_mask(size, rectangles=[(690, 75, 980, 455)]))
    ear_right = intersect(cat, shape_mask(size, rectangles=[(1065, 75, 1370, 455)]))
    eye_left = intersect(cat, shape_mask(size, ellipses=[(785, 350, 985, 535)]))
    eye_right = intersect(cat, shape_mask(size, ellipses=[(1055, 350, 1255, 535)]))
    ear_left = subtract(ear_left, eye_left, eye_right)
    ear_right = subtract(ear_right, eye_left, eye_right)
    upper_left = intersect(eye_left, shape_mask(size, rectangles=[(0, 0, 2048, 420)]))
    upper_right = intersect(eye_right, shape_mask(size, rectangles=[(0, 0, 2048, 420)]))
    lower_left = intersect(eye_left, shape_mask(size, rectangles=[(0, 475, 2048, 2048)]))
    lower_right = intersect(eye_right, shape_mask(size, rectangles=[(0, 475, 2048, 2048)]))
    iris_left = subtract(
        intersect(eye_left, shape_mask(size, ellipses=[(825, 370, 950, 520)])),
        upper_left,
        lower_left,
    )
    iris_right = subtract(
        intersect(eye_right, shape_mask(size, ellipses=[(1090, 370, 1220, 520)])),
        upper_right,
        lower_right,
    )
    eye_white_left = subtract(eye_left, iris_left, upper_left, lower_left)
    eye_white_right = subtract(eye_right, iris_right, upper_right, lower_right)
    muzzle = subtract(
        intersect(cat, shape_mask(size, ellipses=[(745, 500, 1210, 725)])),
        eye_left,
        eye_right,
    )
    leg_left = intersect(cat, shape_mask(size, polygons=[[
        (735, 870), (815, 830), (925, 865), (960, 1030),
        (970, 1320), (985, 1660), (1010, 1875), (955, 1930),
        (705, 1930), (655, 1870), (680, 1630), (690, 1320),
        (700, 1060),
    ]]))
    leg_right = intersect(cat, shape_mask(size, polygons=[[
        (925, 865), (1040, 830), (1145, 875), (1170, 1060),
        (1185, 1320), (1195, 1630), (1225, 1870), (1175, 1930),
        (955, 1930), (910, 1875), (930, 1660), (940, 1320),
        (945, 1030),
    ]]))
    leg_right = subtract(leg_right, leg_left)
    tail = subtract(
        intersect(
            cat,
            shape_mask(
                size,
                polygons=[[
                    (1110, 1510), (1250, 1440), (1385, 1400), (1450, 1320), (1480, 1150),
                    (1510, 1020), (1690, 1020), (1850, 1120), (1880, 1500),
                    (1840, 1700), (1660, 1840), (1390, 1870), (1200, 1800),
                    (1135, 1680),
                ]],
            ),
        ),
        leg_left,
        leg_right,
    )

    detachable = union(
        ear_left, ear_right, eye_left, eye_right, muzzle, leg_left, leg_right, tail
    )
    face_core = union(eye_left, eye_right, muzzle)
    legs_core = union(leg_left, leg_right)
    # reserve 是活动件下方的完整连续底图；正常状态完全被母版切层遮住，
    # 因而无需再次与母版洞口混合。局部混合会反而制造色块接缝。
    underpainting_rgb = warped_rgb
    complete_underpainting_alpha = ImageChops.lighter(cat, warped_subject)
    candidate_cut(underpainting_rgb, complete_underpainting_alpha).save(
        WORK / "标准猫-A-v1-完整底图.png"
    )
    base_alpha = subtract(cat, detachable)
    head_zone = shape_mask(size, rectangles=[(0, 0, 2048, 819)])
    chest_zone = shape_mask(size, rectangles=[(650, 820, 1370, 1599)])
    head_alpha = intersect(base_alpha, head_zone)
    chest_alpha = intersect(base_alpha, chest_zone)
    body_alpha = subtract(base_alpha, head_alpha, chest_alpha)

    # 补画范围必须跟随尾巴的真实曲线。矩形裁切会在尾根移动后露出直边，
    # 因而这里用尾巴轮廓的椭圆膨胀区，并把根部 reserve 限制在弧形区域内。
    tail_motion_envelope = dilate(tail, 64)
    tail_root_curve = intersect(
        dilate(tail, 36),
        shape_mask(size, ellipses=[(960, 1200, 1420, 1840)]),
    )
    tail_body_zone = shape_mask(size, ellipses=[(960, 1200, 1420, 1880)])
    body_under_tail = intersect(warped_subject, tail_motion_envelope, tail_body_zone)
    tail_reserve = intersect(warped_subject, tail_root_curve, tail_body_zone)
    body_under_tail = intersect(body_under_tail, cat)
    tail_reserve = intersect(tail_reserve, cat)
    # 参考图本身是不带耳朵的连续身体；直接使用其平滑后的最大连通 alpha，
    # 避免对耳朵多边形做布尔减法而制造新的直边。
    reserve_subject = warped_subject
    # 同一 reserve 层可以包含多个互不相连的补画岛，但不能覆盖正常身体。
    # 运行时只会在活动件移动后显示这些局部岛；把完整底图整层放在主体上方
    # 会令未移动区域也换成参考图纹理，形成肉眼可见的垂直接缝。
    ear_reserve_zone = union(ear_left, ear_right)
    eye_reserve_zone = union(eye_left, eye_right)
    muzzle_reserve_zone = muzzle
    leg_reserve_zone = legs_core
    face_occlusion_zone = union(
        eye_reserve_zone,
        muzzle_reserve_zone,
    )
    occlusion_zone = union(ear_reserve_zone, face_occlusion_zone, leg_reserve_zone)
    # 耳朵移除后需要采用“无耳参考”的圆润头盖轮廓；其余洞口则必须完整填满，
    # 不能被参考主体 alpha 在洞内截断。
    ear_head_cap = intersect(cat, ear_reserve_zone, reserve_subject)
    face_reserve = intersect(cat, face_occlusion_zone)
    leg_body_silhouette = intersect(cat, leg_reserve_zone, reserve_subject)
    occlusion_reserve = union(ear_head_cap, face_reserve, leg_body_silhouette)
    master_rgb = np.asarray(master)[:, :, :3]
    occlusion_underpainting_rgb = master_rgb.copy()
    # 各类活动件必须独立计算到自身边界的距离。若按遮罩并集计算，彼此相邻的
    # 耳区会让眼睛上缘被误判成“洞口深处”，从而在眼睑边缘提前混入候选色。
    for local_zone in (
        ear_reserve_zone,
        eye_reserve_zone,
        muzzle_reserve_zone,
        leg_reserve_zone,
    ):
        local_rgb = blend_inside_holes(master_rgb, underpainting_rgb, local_zone)
        local_pixels = np.asarray(local_zone) > 0
        occlusion_underpainting_rgb[local_pixels] = local_rgb[local_pixels]
    tail_underpainting_rgb = blend_inside_holes(
        master_rgb,
        underpainting_rgb,
        intersect(tail, body_under_tail),
    )

    files: list[tuple[str, str, bool]] = [
        ("00-body.png", "body", True),
        ("02-chest.png", "chest", True),
        ("03-head.png", "head", True),
        ("01-bodyUnderTail.png", "bodyUnderTail", False),
        ("18-tailReserve.png", "tailReserve", False),
        ("19-occlusionReserve.png", "occlusionReserve", False),
        ("04-earLeft.png", "earLeft", True),
        ("05-earRight.png", "earRight", True),
        ("06-eyeWhiteLeft.png", "eyeWhiteLeft", True),
        ("07-eyeWhiteRight.png", "eyeWhiteRight", True),
        ("08-irisLeft.png", "irisLeft", True),
        ("09-irisRight.png", "irisRight", True),
        ("10-upperLidLeft.png", "upperLidLeft", True),
        ("11-upperLidRight.png", "upperLidRight", True),
        ("12-lowerLidLeft.png", "lowerLidLeft", True),
        ("13-lowerLidRight.png", "lowerLidRight", True),
        ("14-muzzle.png", "muzzle", True),
        ("15-frontLegLeft.png", "frontLegLeft", True),
        ("16-frontLegRight.png", "frontLegRight", True),
        ("17-tail.png", "tail", True),
    ]
    master_layers = {
        "00-body.png": body_alpha,
        "02-chest.png": chest_alpha,
        "03-head.png": head_alpha,
        "04-earLeft.png": ear_left,
        "05-earRight.png": ear_right,
        "06-eyeWhiteLeft.png": eye_white_left,
        "07-eyeWhiteRight.png": eye_white_right,
        "08-irisLeft.png": iris_left,
        "09-irisRight.png": iris_right,
        "10-upperLidLeft.png": upper_left,
        "11-upperLidRight.png": upper_right,
        "12-lowerLidLeft.png": lower_left,
        "13-lowerLidRight.png": lower_right,
        "14-muzzle.png": muzzle,
        "15-frontLegLeft.png": leg_left,
        "16-frontLegRight.png": leg_right,
        "17-tail.png": tail,
    }
    for filename, alpha in master_layers.items():
        cut(master, alpha).save(WORK / filename)
    candidate_cut(tail_underpainting_rgb, body_under_tail).save(WORK / "01-bodyUnderTail.png")
    candidate_cut(tail_underpainting_rgb, tail_reserve).save(WORK / "18-tailReserve.png")
    candidate_cut(occlusion_underpainting_rgb, occlusion_reserve).save(WORK / "19-occlusionReserve.png")

    visible = [filename for filename, _, is_visible in files if is_visible]
    neutral = compose(visible, EVIDENCE / "01-中性重组.png")
    difference = ImageChops.difference(master, neutral)
    difference.save(EVIDENCE / "02-中性重组差异.png")

    evidence_reserves = {
        "eyes": intersect(occlusion_reserve, eye_reserve_zone),
        "ears": intersect(occlusion_reserve, ear_reserve_zone),
        "muzzle": intersect(occlusion_reserve, muzzle_reserve_zone),
        "legs": intersect(occlusion_reserve, leg_reserve_zone),
    }
    evidence_reserve_files = {}
    for key, alpha in evidence_reserves.items():
        path = WORK / f".验收-{key}-遮挡补画.png"
        candidate_cut(occlusion_underpainting_rgb, alpha).save(path)
        evidence_reserve_files[key] = path
    # “完全隐藏尾巴”证据只填回被尾巴实际遮住的轮廓；扩大后的 reserve
    # 专供尾巴摆动极值使用，不能整层显示在中性轮廓上。
    tail_hidden_fill = WORK / ".验收-尾巴移除补画.png"
    candidate_cut(tail_underpainting_rgb, intersect(tail, body_under_tail)).save(tail_hidden_fill)
    tail_below = [tail_hidden_fill.name]
    base = ["00-body.png", "02-chest.png", "03-head.png"]
    detachable_visible = [name for name in visible if name not in set(base)]
    without_eyes = base + [evidence_reserve_files["eyes"].name] + [name for name in detachable_visible if name not in {
        "10-upperLidLeft.png", "11-upperLidRight.png", "12-lowerLidLeft.png", "13-lowerLidRight.png",
    }]
    without_ears = base + [evidence_reserve_files["ears"].name] + [name for name in detachable_visible if name not in {"04-earLeft.png", "05-earRight.png"}]
    without_muzzle = base + [evidence_reserve_files["muzzle"].name] + [name for name in detachable_visible if name != "14-muzzle.png"]
    without_legs = base + [evidence_reserve_files["legs"].name] + [name for name in detachable_visible if name not in {"15-frontLegLeft.png", "16-frontLegRight.png"}]
    without_tail = base + tail_below + [name for name in detachable_visible if name != "17-tail.png"]
    compose(without_eyes, EVIDENCE / "03-隐藏双眼.png")
    compose(without_ears, EVIDENCE / "04-隐藏双耳.png")
    compose(without_muzzle, EVIDENCE / "05-隐藏口鼻.png")
    compose(without_legs, EVIDENCE / "06-隐藏前肢.png")
    compose(without_tail, EVIDENCE / "07-隐藏尾巴.png")
    for path in evidence_reserve_files.values():
        path.unlink(missing_ok=True)
    tail_hidden_fill.unlink(missing_ok=True)
    write_ora(files, EVIDENCE / "01-中性重组.png")

    changed = int(np.count_nonzero(np.asarray(difference)))
    print(f"alignment_inliers={inliers}")
    print(f"alignment_median_error={median_error:.3f}")
    print(f"neutral_difference_values={changed}")
    print(f"body_under_tail_alpha={np.count_nonzero(np.asarray(body_under_tail))}")
    print(f"tail_reserve_alpha={np.count_nonzero(np.asarray(tail_reserve))}")
    print(f"occlusion_reserve_alpha={np.count_nonzero(np.asarray(occlusion_reserve))}")


if __name__ == "__main__":
    main()
