import hashlib
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Final, Literal, assert_never


PIXEL_STYLE_V1_ID: Final = "pixel-style-v1"
PIXEL_STYLE_V2_ID: Final = "pixel-style-v2-animation-ready"
DEFAULT_PIXEL_STYLE_ID: Final = PIXEL_STYLE_V2_ID
SUPPORTED_PIXEL_STYLE_IDS: Final = frozenset({PIXEL_STYLE_V1_ID, PIXEL_STYLE_V2_ID})


class PixelStyleError(ValueError):
    """Raised when the checked-in pixel style pack is invalid."""


@dataclass(frozen=True, slots=True)
class PixelPostprocessProfile:
    logical_grid_size: int
    palette_color_limit: int
    quantize_method: Literal["mediancut", "maxcoverage"]
    dither: Literal["none"]
    protected_accent_slots: int
    safe_margin_ratio: float
    downsample: Literal["box"]
    upsample: Literal["nearest"]


@dataclass(frozen=True, slots=True)
class PixelStylePack:
    profile: dict[str, Any]
    profile_sha256: str
    reference_path: Path
    reference_sha256: str
    postprocess: PixelPostprocessProfile

    @property
    def style_profile_id(self) -> str:
        return self.profile["styleProfileId"]

    @property
    def prompt_contract(self) -> str:
        contract_path = self.reference_path.with_name("提示词合同.md")
        return contract_path.read_text(encoding="utf-8")

    @property
    def prompt_template_version(self) -> str:
        return f"{self.style_profile_id}-prompt-v{self.profile['version']}"

    def prompt_fragment(self) -> str:
        return (
            "PetBaby pixel-style-v1: 16-bit pixel art game sprite, chunky visible pixels, "
            "hard edges with no anti-aliasing, limited color palette with flat shading. "
            "balanced chibi head at 38%-42% of total height, compact seated three-quarter front view, "
            "transparent background. Preserve the subject's face shape, eye color, ear shape, face markings, "
            "chest fur, paw socks, body patches, and tail. One complete pet PNG only; no patches, layers, "
            "parts, rigging, gradients, mosaic filter, or generic cat replacement."
        )


def load_pixel_style_pack(
    style_profile_id: str | Path = DEFAULT_PIXEL_STYLE_ID,
    root: Path | None = None,
) -> PixelStylePack:
    match style_profile_id:
        case Path() as legacy_root:
            selected_style_id = legacy_root.name
            asset_root = root or legacy_root
        case str() as selected_style_id:
            asset_root = root or Path(__file__).parent / "assets" / selected_style_id
        case unreachable:
            assert_never(unreachable)

    if selected_style_id not in SUPPORTED_PIXEL_STYLE_IDS:
        raise PixelStyleError(f"unsupported pixel style: {selected_style_id}")

    profile_path = asset_root / "风格档案.json"
    reference_path = asset_root / "认可参考图.png"
    if not profile_path.is_file() or not reference_path.is_file():
        raise PixelStyleError(f"{selected_style_id} asset pack is incomplete")
    profile_bytes = profile_path.read_bytes()
    try:
        profile = json.loads(profile_bytes.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise PixelStyleError(f"{selected_style_id} profile is invalid JSON") from exc
    if not isinstance(profile, dict):
        raise PixelStyleError(f"{selected_style_id} profile must be an object")
    _validate_profile(profile, selected_style_id)
    reference_bytes = reference_path.read_bytes()
    if not reference_bytes.startswith(b"\x89PNG\r\n\x1a\n"):
        raise PixelStyleError(f"{selected_style_id} reference must be PNG")
    match selected_style_id:
        case "pixel-style-v1":
            postprocess = PixelPostprocessProfile(
                logical_grid_size=512,
                palette_color_limit=32,
                quantize_method="mediancut",
                dither="none",
                protected_accent_slots=0,
                safe_margin_ratio=0.04,
                downsample="box",
                upsample="nearest",
            )
        case "pixel-style-v2-animation-ready":
            postprocess = PixelPostprocessProfile(
                logical_grid_size=160,
                palette_color_limit=24,
                quantize_method="maxcoverage",
                dither="none",
                protected_accent_slots=4,
                safe_margin_ratio=0.06,
                downsample="box",
                upsample="nearest",
            )
        case unreachable:
            assert_never(unreachable)
    return PixelStylePack(
        profile=profile,
        profile_sha256=hashlib.sha256(profile_bytes).hexdigest(),
        reference_path=reference_path,
        reference_sha256=hashlib.sha256(reference_bytes).hexdigest(),
        postprocess=postprocess,
    )


def _validate_profile(profile: dict[str, Any], style_profile_id: str) -> None:
    match style_profile_id:
        case "pixel-style-v1":
            _validate_v1_profile(profile)
        case "pixel-style-v2-animation-ready":
            _validate_v2_profile(profile)
        case unreachable:
            assert_never(unreachable)


def _validate_v1_profile(profile: dict[str, Any]) -> None:
    if profile.get("styleProfileId") != PIXEL_STYLE_V1_ID:
        raise PixelStyleError("styleProfileId must be pixel-style-v1")
    if profile.get("version") != 1 or profile.get("route") != "pixel-v1":
        raise PixelStyleError("pixel-style-v1 version or route is invalid")
    if profile.get("provider") != "lk888" or profile.get("model") != "gpt-image-2":
        raise PixelStyleError("pixel-style-v1 provider model is invalid")
    bounds = profile.get("headShareOfHeight")
    if bounds != [0.38, 0.42]:
        raise PixelStyleError("pixel-style-v1 head ratio is invalid")
    if profile.get("background") != "transparent":
        raise PixelStyleError("pixel-style-v1 background must be transparent")
    for key in ("preserveTraits", "forbidden"):
        values = profile.get(key)
        if not isinstance(values, list) or not values or not all(isinstance(value, str) for value in values):
            raise PixelStyleError(f"pixel-style-v1 {key} is invalid")


def _validate_v2_profile(profile: dict[str, Any]) -> None:
    expected = {
        "styleProfileId": PIXEL_STYLE_V2_ID,
        "version": 2,
        "route": "pixel-v1",
        "provider": "lk888",
        "model": "gpt-image-2",
        "modelDisplayName": "GPT-image-2.0",
        "visualLanguage": "animation-first simplified pixel character with strong silhouette, large deliberate color clusters, and two-step shading",
        "headShareOfHeight": [0.38, 0.42],
        "bodyShareOfHeight": [0.58, 0.62],
        "pose": "stable centered seated front to slight three-quarter view",
        "background": "transparent",
        "safeMarginRatio": 0.06,
        "logicalGridSize": 160,
        "paletteColorLimit": 24,
        "quantizeMethod": "maxcoverage",
        "dither": "none",
        "protectedAccentSlots": 4,
        "downsample": "box",
        "upsample": "nearest",
        "preserveTraits": [
            "face shape and proportions",
            "ear shape",
            "eye color",
            "primary face markings",
            "main chest and paw markings",
            "body silhouette and main patches",
            "tail shape and main markings",
            "signature marks",
        ],
        "forbidden": [
            "microscopic fur texture",
            "secondary texture noise",
            "face patch compositing",
            "body-part assembly",
            "semantic layer output",
            "Live2D rig",
            "generic cat replacement",
            "smooth gradients",
            "anti-aliasing",
        ],
        "referenceAsset": "认可参考图.png",
    }
    if profile != expected:
        raise PixelStyleError("pixel-style-v2-animation-ready profile is invalid")


__all__ = [
    "DEFAULT_PIXEL_STYLE_ID",
    "PIXEL_STYLE_V1_ID",
    "PIXEL_STYLE_V2_ID",
    "PixelPostprocessProfile",
    "PixelStyleError",
    "PixelStylePack",
    "SUPPORTED_PIXEL_STYLE_IDS",
    "load_pixel_style_pack",
]
