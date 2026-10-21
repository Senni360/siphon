"""Canonical, ASCII key names and the two DJ circle-of-fifths notations."""

import math
import re
import string

_MAJOR = ("C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B")
_MINOR = ("C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "Bb", "B")
_CAMELOT = {"major": (8, 3, 10, 5, 12, 7, 2, 9, 4, 11, 6, 1),
            "minor": (5, 12, 7, 2, 9, 4, 11, 6, 1, 8, 3, 10)}
_PITCH = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}


def key_names(tonic: int, mode: str) -> dict:
    """Return all Analysis key fields for pitch class 0=C, ..., 11=B."""
    if isinstance(tonic, bool) or not isinstance(tonic, int) or not 0 <= tonic < 12:
        raise ValueError("tonic must be an integer pitch class in 0..11")
    if mode not in _CAMELOT:
        raise ValueError("mode must be major or minor")
    name = (_MAJOR if mode == "major" else _MINOR)[tonic]
    number = _CAMELOT[mode][tonic]
    return {"key": f"{name} {mode}", "keyShort": name + ("m" if mode == "minor" else ""),
            "tonic": name, "mode": mode, "camelot": f"{number}{'A' if mode == 'minor' else 'B'}",
            "openKey": f"{(number + 4) % 12 + 1}{'m' if mode == 'minor' else 'd'}"}


def parse_key(value: str) -> tuple[int, str]:
    """Parse names (including enharmonics/Unicode), Camelot, or Open Key."""
    if not isinstance(value, str):
        raise ValueError("key must be a string")
    value = value.strip().replace("♯", "#").replace("♭", "b")
    dj = re.fullmatch(r"(1[0-2]|[1-9])([ABmd])", value)
    if dj:
        number, suffix = int(dj[1]), dj[2]
        mode = "minor" if suffix in ("A", "m") else "major"
        if suffix in ("m", "d"):
            number = (number + 6) % 12 + 1
        return _CAMELOT[mode].index(number), mode
    match = re.fullmatch(r"([A-Ga-g])([#b]?)(?:\s*(major|minor|maj|min|m|M))?", value)
    if not match:
        raise ValueError(f"Invalid musical key: {value!r}")
    pitch = (_PITCH[match[1].upper()] + {"": 0, "#": 1, "b": -1}[match[2]]) % 12
    return pitch, "minor" if match[3] in ("minor", "min", "m") else "major"


def to_camelot(key: str) -> str:
    return key_names(*parse_key(key))["camelot"]


def from_camelot(value: str, *, short: bool = False) -> str:
    if not re.fullmatch(r"(1[0-2]|[1-9])[AB]", value):
        raise ValueError("Expected a Camelot key, e.g. 8A")
    return key_names(*parse_key(value))["keyShort" if short else "key"]


def to_open_key(key: str) -> str:
    return key_names(*parse_key(key))["openKey"]


def from_open_key(value: str, *, short: bool = False) -> str:
    if not re.fullmatch(r"(1[0-2]|[1-9])[md]", value):
        raise ValueError("Expected an Open Key, e.g. 1m")
    return key_names(*parse_key(value))["keyShort" if short else "key"]


def format_suffix(analysis: dict, template: str = " [{bpm} BPM {key}]", notation: str = "standard") -> str:
    """Format only supported placeholders, then make a Windows-safe suffix.

    The leading space in the default template is intentional. This is a suffix,
    not an entire filename: the caller handles total path length and collisions.
    Unknown placeholders/format operations raise ValueError rather than evaluating
    arbitrary field access. The suffix is bounded to 160 characters.
    """
    if notation not in ("standard", "camelot", "openkey"):
        raise ValueError("notation must be standard, camelot, or openkey")
    bpm = float(analysis["bpm"])
    if not math.isfinite(bpm) or bpm <= 0:
        raise ValueError("bpm must be finite and positive")
    names = key_names(*parse_key(analysis.get("keyShort") or analysis["key"]))
    # {bpm} is a whole number (filenames read better); {bpmExact} keeps one decimal.
    fields = {"bpm": str(math.floor(bpm + 0.5)),
              "bpmExact": str(round(bpm)) if abs(bpm - round(bpm)) < 0.05 else f"{bpm:.1f}",
              "keyShort": names["keyShort"], "keyLong": names["key"],
              "camelot": names["camelot"], "openkey": names["openKey"]}
    fields["key"] = fields[{"standard": "keyShort", "camelot": "camelot", "openkey": "openkey"}[notation]]
    for _, field, spec, conversion in string.Formatter().parse(template):
        if field is not None and (field not in fields or spec or conversion):
            raise ValueError(f"Unsupported suffix placeholder: {field!r}")
    result = template.format_map(fields)
    result = re.sub(r'[<>:"/\\|?*\x00-\x1f\x7f]', "_", result)
    return result[:160].rstrip(" .")
