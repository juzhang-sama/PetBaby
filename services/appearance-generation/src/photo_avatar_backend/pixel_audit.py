from collections.abc import Mapping

from .pixel_audit_types import (
    JsonValue,
    PixelAlphaReportV1,
    PixelAuditError,
    audit_integer,
)
from .pixel_audit_v1 import PixelAvatarAuditV1
from .pixel_audit_v2 import PixelAvatarAuditV2


PixelAvatarAudit = PixelAvatarAuditV1 | PixelAvatarAuditV2


def parse_pixel_avatar_audit(raw: JsonValue) -> PixelAvatarAudit:
    if not isinstance(raw, Mapping):
        raise PixelAuditError("pixel avatar audit must be an object")
    schema_version = audit_integer(
        raw.get("schemaVersion"),
        "pixel avatar audit schemaVersion",
        minimum=1,
        maximum=2,
    )
    if schema_version == 1:
        return PixelAvatarAuditV1.from_wire(raw)
    return PixelAvatarAuditV2.from_wire(raw)


__all__ = [
    "JsonValue",
    "PixelAlphaReportV1",
    "PixelAuditError",
    "PixelAvatarAudit",
    "PixelAvatarAuditV1",
    "PixelAvatarAuditV2",
    "parse_pixel_avatar_audit",
]
