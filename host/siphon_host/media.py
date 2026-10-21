"""Keep extractor data small and JSON-safe at the protocol boundary."""

import math
from urllib.parse import quote, urljoin, urlsplit


def number(value):
    return value if isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) else None


def string(value, limit=4096) -> str:
    return str(value or "")[:limit]


def thumbnail(info):
    return string(info.get("thumbnail") or next((t.get("url") for t in reversed(info.get("thumbnails") or []) if t.get("url")), None)) or None


def entry_preview(entry, parent_url):
    result = {"title": string(entry.get("title"), 512), "duration": number(entry.get("duration"))}
    candidates = [entry.get(key) for key in ("webpage_url", "url", "original_url")]
    formats = entry.get("formats") or []
    candidates += [f.get("url") for f in formats if f.get("vcodec") == "none" or f.get("acodec") not in (None, "none")]
    candidates += [f.get("url") for f in formats]
    for value in candidates:
        if not isinstance(value, str) or not value or len(value) > 16384:
            continue
        # A bare extractor id is not a relative URL (e.g. YouTube flat entries).
        candidate = urljoin(parent_url, value) if value.startswith(("/", "./", "../")) else value
        try:
            parts = urlsplit(candidate)
            if parts.scheme in ("http", "https") and parts.hostname and not parts.username and not parts.password:
                result["url"] = candidate
                return result
        except ValueError:
            pass
    identifier = entry.get("id") or entry.get("url")
    if identifier and "youtube" in string(entry.get("ie_key") or entry.get("extractor_key")).lower():
        result["url"] = "https://www.youtube.com/watch?v=" + quote(str(identifier), safe="")
    else:
        # Never invent a playable URL from an opaque id or repeat the parent
        # collection URL: that could download the wrong item or whole playlist.
        result["downloadable"] = False
    return result


def media_info(info: dict, url: str) -> dict:
    playlist = info.get("_type") in ("playlist", "multi_video") or "entries" in info
    result = {
        "url": url, "webpageUrl": string(info.get("webpage_url") or url),
        "extractor": string(info.get("extractor") or info.get("extractor_key")),
        "id": string(info.get("id")), "title": string(info.get("title")),
        "uploader": string(info.get("uploader") or info.get("artist")),
        "duration": number(info.get("duration")), "thumbnail": thumbnail(info),
        "isPlaylist": playlist, "isLive": bool(info.get("is_live") or info.get("live_status") == "is_live"),
        "formatsAudio": [
            {"formatId": string(f.get("format_id")), "ext": string(f.get("ext")), "acodec": string(f.get("acodec")),
             "abr": number(f.get("abr")), "filesize": number(f.get("filesize") or f.get("filesize_approx"))}
            for f in (info.get("formats") or [])[:300] if f.get("acodec") not in (None, "none")
        ],
    }
    if playlist:
        # A 1 MiB host frame cannot carry arbitrarily large playlists. Entry count
        # remains the full extractor count; the preview list is deliberately bounded.
        from itertools import islice
        preview = list(islice((entry for entry in (info.get("entries") or []) if entry), 201))
        entries = preview[:200]
        result["entries"] = [entry_preview(e, result["webpageUrl"]) for e in entries]
        result["entryCount"] = number(info.get("playlist_count") or info.get("n_entries")) or len(entries)
        if len(preview) > 200:
            result["entriesTruncated"] = True
    return result
