import json

from siphon_host.media import media_info
import pytest


def test_playlist_preview_bounded_but_full_count_preserved():
    consumed = []
    def entries():
        for index in range(10000):
            consumed.append(index)
            yield {"url": f"https://example.org/{index}", "title": "Track", "duration": None}
    result = media_info({"_type": "playlist", "entries": entries(), "playlist_count": 10000}, "https://example.org/playlist")
    assert len(result["entries"]) == 200
    assert len(consumed) == 201
    assert result["entryCount"] == 10000
    assert result["entriesTruncated"]
    assert len(json.dumps(result).encode()) < 1024 * 1024


def test_exactly_two_hundred_entries_are_not_truncated():
    result = media_info({"_type": "playlist", "entries": [{"url": "https://example.org/1"}] * 200}, "https://example.org/playlist")
    assert "entriesTruncated" not in result


def test_raw_extractor_thumbnails_and_nonfinite_numbers():
    result = media_info({"thumbnails": [{"url": "https://example.org/cover.jpg"}], "duration": float("inf"), "formats": [
        {"format_id": "opus", "acodec": "opus", "abr": float("nan")}, {"format_id": "video", "acodec": "none"}
    ]}, "https://example.org/video")
    assert result["thumbnail"] == "https://example.org/cover.jpg"
    assert result["duration"] is None
    assert len(result["formatsAudio"]) == 1
    assert result["formatsAudio"][0]["abr"] is None
    json.dumps(result, allow_nan=False)


@pytest.mark.parametrize("entry,expected", [
    ({"webpage_url": "https://example.org/track", "url": "https://example.org/raw"}, "https://example.org/track"),
    ({"original_url": "https://example.org/original"}, "https://example.org/original"),
    ({"formats": [{"url": "https://archive.org/download/book/poem.mp3", "acodec": "mp3", "vcodec": "none"}]}, "https://archive.org/download/book/poem.mp3"),
    ({"url": "/download/book/poem.mp3"}, "https://archive.org/download/book/poem.mp3"),
    ({"id": "abc", "ie_key": "Youtube"}, "https://www.youtube.com/watch?v=abc"),
    ({"url": "abc", "ie_key": "Youtube"}, "https://www.youtube.com/watch?v=abc"),
])
def test_playlist_preview_resolves_playable_urls(entry, expected):
    result = media_info({"entries": [dict(entry, title="Poem", duration=64.4)]}, "https://archive.org/details/book")
    assert result["entries"][0] == {"url": expected, "title": "Poem", "duration": 64.4}


@pytest.mark.parametrize("entry", [{"id": "opaque"}, {"url": "javascript:alert(1)"}, {"url": "https://[invalid"}, {"url": "https://user:secret@example.org/file"}])
def test_unresolvable_preview_omits_url_and_disables_download(entry):
    result = media_info({"entries": [entry]}, "https://example.org/playlist")
    assert "url" not in result["entries"][0]
    assert result["entries"][0]["downloadable"] is False
