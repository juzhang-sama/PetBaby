# -*- coding: utf-8 -*-
"""Locked identity traits structure and prompt block builder."""
from dataclasses import asdict, dataclass, field

VALID_SPECIES = {"cat", "dog"}


@dataclass
class LockedTraits:
    species: str = "cat"
    fur_colors: list[str] = field(default_factory=list)
    pattern: str = ""
    ears: str = ""
    eye_color: str = ""
    face_notes: str = ""

    def validate(self) -> None:
        if self.species not in VALID_SPECIES:
            raise ValueError(f"species must be one of {sorted(VALID_SPECIES)}")

    def to_prompt_block(self) -> dict[str, str]:
        self.validate()
        if not self.fur_colors:
            return {}
        return {
            "species": self.species,
            "fur_colors": ", ".join(self.fur_colors),
            "pattern": self.pattern,
            "ears": self.ears,
            "eye_color": self.eye_color,
            "face_notes": self.face_notes,
        }

    def to_dict(self) -> dict:
        return asdict(self)

    @classmethod
    def from_dict(cls, data: dict) -> "LockedTraits":
        traits = cls(
            species=str(data.get("species", "cat")),
            fur_colors=[str(c) for c in data.get("fur_colors", [])],
            pattern=str(data.get("pattern", "")),
            ears=str(data.get("ears", "")),
            eye_color=str(data.get("eye_color", "")),
            face_notes=str(data.get("face_notes", "")),
        )
        traits.validate()
        return traits
