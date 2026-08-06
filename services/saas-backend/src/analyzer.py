# -*- coding: utf-8 -*-
"""Pet identity analyzer: turns 1-3 reference photos into locked traits.

The analyzer is optional. It calls an OpenAI-compatible chat completions
endpoint (the same aggregation platform as generation) with the photos and
asks for a strict JSON identity profile. When the model is not configured or
the call fails, callers simply fall back to reference-image-only prompts.
"""
import base64
import json

import httpx

ANALYZE_SYSTEM_PROMPT = (
    "You are a pet identity analyst. Look at the reference photo(s) of one pet "
    "and return a JSON object with exactly these keys:\n"
    '{"species": "cat" or "dog", '
    '"fur_colors": ["main coat colors, up to 4"], '
    '"pattern": "coat pattern, e.g. solid/striped/spotted/cow, or empty string", '
    '"ears": "ear shape, e.g. drooping/pointed/tufted, or empty string", '
    '"eye_color": "eye color, or empty string", '
    '"face_notes": "face details that make this pet recognizable: eye shape, nose, '
    'mouth, whiskers, muzzle, face markings, expressions, or empty string"}\n'
    "Focus on the face; it matters most for likeness. Do not describe the "
    "background, pose, or camera. If a detail is unclear leave it empty. "
    "Return only the JSON object, no markdown fences."
)

LANDMARK_SYSTEM_PROMPT = (
    "You are a pet feature annotator. The image shows a single pet on a "
    "transparent or plain background, cropped tightly around the pet. "
    "Return JSON with normalized boxes (0..1 relative to image width/height) "
    "for exactly these keys: leftEye, rightEye, leftEar, rightEar, tail. "
    "Each box is {\"x\": center x, \"y\": center y, \"width\": box width, "
    "\"height\": box height}. Eyes: the visible eye areas including the "
    "surrounding eye region. Ears: the ear areas. Tail: the tail area if "
    "visible, otherwise the best-guess region at the pet's lower side. "
    "All boxes must stay within 0..1. Return only the JSON object."
)


class PetAnalyzer:
    def __init__(self, key: str, base: str, model: str, timeout: float = 60.0):
        self._key = key
        self._base = base.rstrip("/")
        self._model = model
        self._timeout = timeout

    def _chat_json(
        self,
        system_prompt: str,
        user_text: str,
        photos: list[tuple[bytes, str]],
        max_tokens: int = 600,
    ) -> dict | None:
        if not self._model or not photos:
            return None
        try:
            response = httpx.post(
                f"{self._base}/v1/chat/completions",
                headers={
                    "Authorization": f"Bearer {self._key}",
                    "Content-Type": "application/json",
                },
                json={
                    "model": self._model,
                    "messages": [
                        {
                            "role": "system",
                            "content": system_prompt,
                        },
                        {
                            "role": "user",
                            "content": [
                                {
                                    "type": "text",
                                    "text": user_text,
                                },
                                *[
                                    {
                                        "type": "image_url",
                                        "image_url": {
                                            "url": (
                                                f"data:{mime};base64,"
                                                + base64.b64encode(data).decode()
                                            )
                                        },
                                    }
                                    for data, mime in photos[:3]
                                ],
                            ],
                        }
                    ],
                    "temperature": 0,
                    "max_tokens": max_tokens,
                    "response_format": {"type": "json_object"},
                },
                timeout=self._timeout,
            )
        except httpx.HTTPError as exc:
            print(f"[analyzer] request failed, falling back: {exc}", flush=True)
            return None
        if response.status_code != 200:
            print(
                f"[analyzer] returned {response.status_code}, falling back: "
                f"{response.text[:200]}",
                flush=True,
            )
            return None
        try:
            content = response.json()["choices"][0]["message"]["content"]
            data = json.loads(_strip_code_fences(content))
        except (KeyError, IndexError, TypeError, json.JSONDecodeError) as exc:
            print(f"[analyzer] bad response, falling back: {exc}", flush=True)
            return None
        if not isinstance(data, dict):
            return None
        return data

    def analyze(
        self,
        photos: list[tuple[bytes, str]],
        species: str,
    ) -> dict | None:
        data = self._chat_json(
            ANALYZE_SYSTEM_PROMPT,
            f"Species hint: {species}",
            photos,
        )
        return _normalize_traits(data, species) if data is not None else None

    def analyze_landmarks(
        self,
        photo: tuple[bytes, str],
        species: str,
    ) -> dict | None:
        data = self._chat_json(
            LANDMARK_SYSTEM_PROMPT,
            f"Species hint: {species}",
            [photo],
            max_tokens=400,
        )
        return _normalize_landmarks(data) if data is not None else None


def _strip_code_fences(content: str) -> str:
    text = content.strip()
    if text.startswith("```"):
        lines = text.splitlines()
        if lines and lines[0].startswith("```"):
            lines = lines[1:]
        if lines and lines[-1].strip() == "```":
            lines = lines[:-1]
        text = "\n".join(lines).strip()
    return text


def _normalize_traits(raw: dict, species: str) -> dict:
    fur_colors = raw.get("fur_colors")
    if isinstance(fur_colors, str):
        fur_colors = [fur_colors]
    if not isinstance(fur_colors, list):
        fur_colors = []
    return {
        "species": species if species in ("cat", "dog") else "cat",
        "fur_colors": [str(c).strip() for c in fur_colors if str(c).strip()][:4],
        "pattern": str(raw.get("pattern", "") or "").strip(),
        "ears": str(raw.get("ears", "") or "").strip(),
        "eye_color": str(raw.get("eye_color", "") or "").strip(),
        "face_notes": str(raw.get("face_notes", "") or "").strip(),
    }


def _normalize_landmarks(raw: dict) -> dict | None:
    keys = ("leftEye", "rightEye", "leftEar", "rightEar", "tail")
    boxes: dict[str, dict[str, float]] = {}
    for key in keys:
        box = raw.get(key)
        if not isinstance(box, dict):
            return None
        values = [box.get(k) for k in ("x", "y", "width", "height")]
        if not all(isinstance(v, (int, float)) and v >= 0 for v in values):
            return None
        x = min(float(values[0]), 1.0)  # type: ignore[arg-type]
        y = min(float(values[1]), 1.0)  # type: ignore[arg-type]
        # the model sometimes lets a box overhang the image edge slightly;
        # clamp instead of rejecting so a good landmark is never lost
        width = min(float(values[2]), 1.0 - x)  # type: ignore[arg-type]
        height = min(float(values[3]), 1.0 - y)  # type: ignore[arg-type]
        boxes[key] = {
            "x": x,
            "y": y,
            "width": max(width, 0.0),
            "height": max(height, 0.0),
        }
    return boxes
