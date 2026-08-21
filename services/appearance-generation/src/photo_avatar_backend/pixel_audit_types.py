from collections.abc import Mapping
from dataclasses import dataclass


type JsonScalar = None | bool | int | float | str
type JsonValue = JsonScalar | list[JsonValue] | Mapping[str, JsonValue]


_ALPHA_FIELDS = frozenset(
    {
        "visiblePixels", "partialAlphaPixels", "partialAlphaRatio",
        "largestComponentPixels", "largestComponentShare", "boundsLeft",
        "boundsTop", "boundsRight", "boundsBottom", "marginLeft", "marginTop",
        "marginRight", "marginBottom",
    }
)


class PixelAuditError(ValueError):
    pass


@dataclass(frozen=True, slots=True)
class PixelAlphaReportV1:
    visible_pixels: int
    partial_alpha_pixels: int
    partial_alpha_ratio: float
    largest_component_pixels: int
    largest_component_share: float
    bounds_left: int
    bounds_top: int
    bounds_right: int
    bounds_bottom: int
    margin_left: int
    margin_top: int
    margin_right: int
    margin_bottom: int

    def to_wire(self) -> dict[str, JsonValue]:
        return {
            "visiblePixels": self.visible_pixels,
            "partialAlphaPixels": self.partial_alpha_pixels,
            "partialAlphaRatio": self.partial_alpha_ratio,
            "largestComponentPixels": self.largest_component_pixels,
            "largestComponentShare": self.largest_component_share,
            "boundsLeft": self.bounds_left,
            "boundsTop": self.bounds_top,
            "boundsRight": self.bounds_right,
            "boundsBottom": self.bounds_bottom,
            "marginLeft": self.margin_left,
            "marginTop": self.margin_top,
            "marginRight": self.margin_right,
            "marginBottom": self.margin_bottom,
        }

    @classmethod
    def from_wire(cls, raw: JsonValue) -> "PixelAlphaReportV1":
        value = audit_mapping(raw, _ALPHA_FIELDS, "pixel alpha report")
        report = cls(
            visible_pixels=audit_integer(value["visiblePixels"], "visible pixels", minimum=1),
            partial_alpha_pixels=audit_integer(
                value["partialAlphaPixels"], "partial alpha pixels", minimum=0
            ),
            partial_alpha_ratio=audit_ratio(
                value["partialAlphaRatio"], "partial alpha ratio"
            ),
            largest_component_pixels=audit_integer(
                value["largestComponentPixels"], "largest component pixels", minimum=1
            ),
            largest_component_share=audit_ratio(
                value["largestComponentShare"], "largest component share"
            ),
            bounds_left=audit_integer(value["boundsLeft"], "bounds left", minimum=0),
            bounds_top=audit_integer(value["boundsTop"], "bounds top", minimum=0),
            bounds_right=audit_integer(value["boundsRight"], "bounds right", minimum=1),
            bounds_bottom=audit_integer(
                value["boundsBottom"], "bounds bottom", minimum=1
            ),
            margin_left=audit_integer(value["marginLeft"], "margin left", minimum=0),
            margin_top=audit_integer(value["marginTop"], "margin top", minimum=0),
            margin_right=audit_integer(value["marginRight"], "margin right", minimum=0),
            margin_bottom=audit_integer(
                value["marginBottom"], "margin bottom", minimum=0
            ),
        )
        if report.partial_alpha_pixels > report.visible_pixels:
            raise PixelAuditError("partial alpha pixels exceed visible pixels")
        if report.largest_component_pixels > report.visible_pixels:
            raise PixelAuditError("largest component pixels exceed visible pixels")
        if abs(report.partial_alpha_ratio - report.partial_alpha_pixels / report.visible_pixels) > 1e-9:
            raise PixelAuditError("partial alpha ratio is inconsistent")
        if abs(
            report.largest_component_share
            - report.largest_component_pixels / report.visible_pixels
        ) > 1e-9:
            raise PixelAuditError("largest component share is inconsistent")
        return report


def audit_mapping(
    raw: JsonValue,
    fields: frozenset[str],
    label: str,
) -> Mapping[str, JsonValue]:
    if not isinstance(raw, Mapping) or set(raw) != fields:
        raise PixelAuditError(f"{label} fields are invalid")
    return raw


def audit_text(value: JsonValue, label: str) -> str:
    if not isinstance(value, str) or not value or len(value) > 256:
        raise PixelAuditError(f"{label} is invalid")
    return value


def audit_integer(
    value: JsonValue,
    label: str,
    *,
    minimum: int,
    maximum: int | None = None,
) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < minimum:
        raise PixelAuditError(f"{label} is invalid")
    if maximum is not None and value > maximum:
        raise PixelAuditError(f"{label} is invalid")
    return value


def audit_ratio(value: JsonValue, label: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise PixelAuditError(f"{label} is invalid")
    ratio = float(value)
    if not 0.0 <= ratio <= 1.0:
        raise PixelAuditError(f"{label} is invalid")
    return ratio


def audit_sha(value: JsonValue) -> str:
    text = audit_text(value, "sha256")
    if len(text) != 64 or any(character not in "0123456789abcdef" for character in text):
        raise PixelAuditError("sha256 is invalid")
    return text


def validate_alpha_bounds(
    alpha: PixelAlphaReportV1,
    width: int,
    height: int,
) -> None:
    if (
        alpha.bounds_right > width
        or alpha.bounds_bottom > height
        or alpha.margin_left != alpha.bounds_left
        or alpha.margin_top != alpha.bounds_top
        or alpha.margin_right != width - alpha.bounds_right
        or alpha.margin_bottom != height - alpha.bounds_bottom
    ):
        raise PixelAuditError("pixel avatar alpha bounds are inconsistent")
