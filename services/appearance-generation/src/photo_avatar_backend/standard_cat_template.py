"""Load and verify the immutable standard cat body template.

标准猫体模板是「生成规整 + 动作库 + 未来 Live2D 绑定」的公共基础设施：
- proportions：体型比例（1.8 头身标准坐姿）——供生成规整贴合。
- parts：分层部件（head/body/tail/paws）——供像素动作引擎按层驱动。
- space：动作空间锚点（eyes/earRoots/breathZone/tailRoot/...），语义兼容
  Live2D 路线的 MotionSpatialProfileV1 与像素 motion-profile.json。
- amplitude：动作幅度（breath/blink/ear/tailAngle/tailCurl/tailTip/bodyStretch）。

坐标一律归一化（0..1），配合 canvas 换算像素。
"""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, assert_never

from .pixel_style import DEFAULT_PIXEL_STYLE_ID


class StandardCatTemplateError(ValueError):
    """Raised when the checked-in standard cat template is invalid."""


AMPLITUDE_SEMANTICS: tuple[str, ...] = (
    "breath",
    "blink",
    "ear",
    "tailAngle",
    "tailCurl",
    "tailTip",
    "bodyStretch",
)

PART_IDS: tuple[str, ...] = ("tail", "paws", "body", "head")

_SPACE_RECT_KEYS: tuple[str, ...] = ("alphaBounds", "faceSafeZone", "breathZone", "edgeTailBounds")


@dataclass(frozen=True, slots=True)
class StandardCatTemplate:
    data: dict[str, Any]
    template_sha256: str

    @property
    def template_id(self) -> str:
        return str(self.data["templateId"])

    @property
    def engine_profile(self) -> str:
        return str(self.data["engineProfile"])

    def amplitude(self, semantic: str) -> tuple[float, float]:
        entry = self.data["amplitude"][semantic]
        return float(entry["min"]), float(entry["max"])


def load_standard_cat_template(
    style_profile_id: str | Path = DEFAULT_PIXEL_STYLE_ID,
    root: Path | None = None,
) -> StandardCatTemplate:
    match style_profile_id:
        case Path() as legacy_root:
            asset_root = root or legacy_root
        case ("pixel-style-v1" | "pixel-style-v2-animation-ready") as selected_style_id:
            asset_root = root or Path(__file__).parent / "assets" / selected_style_id
        case str() as unsupported_style_id:
            raise StandardCatTemplateError(f"unsupported pixel style: {unsupported_style_id}")
        case unreachable:
            assert_never(unreachable)
    template_path = asset_root / "标准猫体模板.json"
    if not template_path.is_file():
        raise StandardCatTemplateError("standard cat template is missing")
    template_bytes = template_path.read_bytes()
    try:
        data = json.loads(template_bytes.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise StandardCatTemplateError("standard cat template is invalid JSON") from exc
    if not isinstance(data, dict):
        raise StandardCatTemplateError("standard cat template must be an object")
    _validate_template(data)
    return StandardCatTemplate(
        data=data,
        template_sha256=hashlib.sha256(template_bytes).hexdigest(),
    )


def _validate_template(data: dict[str, Any]) -> None:
    if data.get("schemaVersion") != 1:
        raise StandardCatTemplateError("standard cat template schemaVersion must be 1")
    if not isinstance(data.get("templateId"), str) or not data["templateId"]:
        raise StandardCatTemplateError("templateId must be a non-empty string")
    if not isinstance(data.get("version"), int) or data["version"] < 1:
        raise StandardCatTemplateError("version must be a positive integer")
    if not isinstance(data.get("engineProfile"), str) or not data["engineProfile"]:
        raise StandardCatTemplateError("engineProfile must be a non-empty string")

    canvas = data.get("canvas")
    if (
        not isinstance(canvas, dict)
        or not isinstance(canvas.get("width"), int)
        or not isinstance(canvas.get("height"), int)
        or canvas["width"] <= 0
        or canvas["height"] <= 0
    ):
        raise StandardCatTemplateError("canvas must have positive integer width and height")

    _validate_proportions(data.get("proportions"))
    _validate_parts(data.get("parts"))
    _validate_space(data.get("space"))
    _validate_amplitude(data.get("amplitude"))


def _validate_proportions(proportions: Any) -> None:
    if not isinstance(proportions, dict):
        raise StandardCatTemplateError("proportions must be an object")
    required = ("headHeightFraction", "bodyHeightFraction", "headWidthFraction", "limbHeightFraction")
    for key in required:
        value = proportions.get(key)
        if not isinstance(value, (int, float)) or not (0.0 < value < 1.0):
            raise StandardCatTemplateError(f"proportions.{key} must be a fraction in (0, 1)")
    head = float(proportions["headHeightFraction"])
    body = float(proportions["bodyHeightFraction"])
    if abs(head + body - 1.0) > 0.05:
        raise StandardCatTemplateError(
            f"headHeightFraction + bodyHeightFraction must be ~1, got {head + body:.3f}"
        )


def _validate_parts(parts: Any) -> None:
    if not isinstance(parts, list) or not parts:
        raise StandardCatTemplateError("parts must be a non-empty array")
    ids: set[str] = set()
    layers: set[int] = set()
    for index, part in enumerate(parts):
        if not isinstance(part, dict):
            raise StandardCatTemplateError(f"parts[{index}] must be an object")
        part_id = part.get("id")
        if not isinstance(part_id, str) or not part_id:
            raise StandardCatTemplateError(f"parts[{index}].id must be a non-empty string")
        if part_id in ids:
            raise StandardCatTemplateError(f"parts[{index}].id duplicated: {part_id}")
        ids.add(part_id)
        layer = part.get("layer")
        if not isinstance(layer, int) or layer < 0:
            raise StandardCatTemplateError(f"parts[{index}].layer must be a non-negative integer")
        if layer in layers:
            raise StandardCatTemplateError(f"parts[{index}].layer duplicated: {layer}")
        layers.add(layer)
        _validate_rect(part.get("bounds"), f"parts[{index}].bounds")


def _validate_space(space: Any) -> None:
    if not isinstance(space, dict):
        raise StandardCatTemplateError("space must be an object")
    for key in _SPACE_RECT_KEYS:
        _validate_rect(space.get(key), f"space.{key}")
    eyes = space.get("eyes")
    if not isinstance(eyes, dict):
        raise StandardCatTemplateError("space.eyes must be an object")
    for side in ("left", "right"):
        eye = eyes.get(side)
        if not isinstance(eye, dict):
            raise StandardCatTemplateError(f"space.eyes.{side} must be an object")
        _validate_point(eye.get("center"), f"space.eyes.{side}.center")
        _validate_rect(eye.get("bounds"), f"space.eyes.{side}.bounds")
    ear_roots = space.get("earRoots")
    if not isinstance(ear_roots, dict):
        raise StandardCatTemplateError("space.earRoots must be an object")
    for side in ("left", "right"):
        _validate_point(ear_roots.get(side), f"space.earRoots.{side}")
    stretch = space.get("stretchAxis")
    if not isinstance(stretch, dict):
        raise StandardCatTemplateError("space.stretchAxis must be an object")
    _validate_point(stretch.get("origin"), "space.stretchAxis.origin")
    direction = stretch.get("direction")
    if not isinstance(direction, dict):
        raise StandardCatTemplateError("space.stretchAxis.direction must be an object")
    for key in ("x", "y"):
        value = direction.get(key)
        if not isinstance(value, (int, float)) or not (-1.0 <= float(value) <= 1.0):
            raise StandardCatTemplateError(f"space.stretchAxis.direction.{key} must be in [-1, 1]")
    _validate_point(space.get("swayPivot"), "space.swayPivot")
    _validate_point(space.get("tailRoot"), "space.tailRoot")


def _validate_amplitude(amplitude: Any) -> None:
    if not isinstance(amplitude, dict):
        raise StandardCatTemplateError("amplitude must be an object")
    for semantic in AMPLITUDE_SEMANTICS:
        entry = amplitude.get(semantic)
        if not isinstance(entry, dict):
            raise StandardCatTemplateError(f"amplitude.{semantic} must be an object")
        low = entry.get("min")
        high = entry.get("max")
        if not isinstance(low, (int, float)) or not isinstance(high, (int, float)):
            raise StandardCatTemplateError(f"amplitude.{semantic} min/max must be numbers")
        if float(low) > float(high):
            raise StandardCatTemplateError(f"amplitude.{semantic} min must not exceed max")


def _validate_point(point: Any, label: str) -> None:
    if not isinstance(point, dict):
        raise StandardCatTemplateError(f"{label} must be an object")
    for key in ("x", "y"):
        value = point.get(key)
        if not isinstance(value, (int, float)) or not (0.0 <= float(value) <= 1.0):
            raise StandardCatTemplateError(f"{label}.{key} must be normalized in [0, 1]")


def _validate_rect(rect: Any, label: str) -> None:
    if not isinstance(rect, dict):
        raise StandardCatTemplateError(f"{label} must be an object")
    for key in ("left", "top", "right", "bottom"):
        value = rect.get(key)
        if not isinstance(value, (int, float)) or not (0.0 <= float(value) <= 1.0):
            raise StandardCatTemplateError(f"{label}.{key} must be normalized in [0, 1]")
    if not (rect["left"] < rect["right"] and rect["top"] < rect["bottom"]):
        raise StandardCatTemplateError(f"{label} must satisfy left<right and top<bottom")


__all__ = [
    "AMPLITUDE_SEMANTICS",
    "PART_IDS",
    "StandardCatTemplate",
    "StandardCatTemplateError",
    "load_standard_cat_template",
]
