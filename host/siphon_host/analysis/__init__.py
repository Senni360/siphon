"""Siphon's lazy-loading, local tempo and musical-key analysis API."""

from .notation import (format_suffix, from_camelot, from_open_key, key_names,
                       parse_key, to_camelot, to_open_key)
from .tags import write_acid, write_tags


class AnalysisError(ValueError):
    """Audio cannot yield a meaningful estimate, or could not be decoded."""


def analyze_file(path, *, max_seconds=None, bpm_range=(70, 180)) -> dict:
    """Decode local audio and return the JSON-compatible Analysis from SPEC.md.

    None uses at most the middle 180 seconds. Numeric/scientific imports happen
    only on this call. Decode failures raise AnalysisError; invalid options raise
    ValueError, and filesystem failures retain their OSError type. No stdout logs.
    """
    from .engine import analyze
    return analyze(path, max_seconds=max_seconds, bpm_range=bpm_range)


__all__ = ["AnalysisError", "analyze_file", "format_suffix", "write_tags", "write_acid", "key_names",
           "parse_key", "to_camelot", "from_camelot", "to_open_key", "from_open_key"]
