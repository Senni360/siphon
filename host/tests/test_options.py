import logging
from pathlib import Path

import pytest

from siphon_host.downloader import create_downloader
from siphon_host.errors import HostError
from siphon_host.options import build_download_params, cookie_params, parse_rate_limit, validate_options, validate_url


def params(options=None):
    return build_download_params(options or {}, logging.getLogger("test"), {"path": None}, {"path": "node"})


def test_defaults_metadata_artwork_and_runtime():
    value = params()
    assert value["format"] == "bestaudio/best"
    assert value["js_runtimes"] == {"node": {"path": "node"}}
    assert value["quiet"] and value["noprogress"]
    assert value["socket_timeout"] == 20
    assert value["windowsfilenames"] and not value["restrictfilenames"]
    assert value["noplaylist"] and not value["overwrites"]
    keys = [pp["key"] for pp in value["postprocessors"]]
    assert keys == ["SponsorBlock", "FFmpegThumbnailsConvertor", "FFmpegExtractAudio", "ModifyChapters", "FFmpegMetadata", "EmbedThumbnail"]
    assert value["postprocessors"][0]["categories"] == ["music_offtopic"]
    metadata = next(pp for pp in value["postprocessors"] if pp["key"] == "FFmpegMetadata")
    assert metadata["add_metadata"] and not metadata["add_chapters"]


@pytest.mark.parametrize("fmt", ["mp3", "m4a", "opus", "flac", "wav", "ogg", "best"])
def test_formats_and_v0(fmt):
    value = params({"format": fmt, "quality": "V0"})
    audio = next(pp for pp in value["postprocessors"] if pp["key"] == "FFmpegExtractAudio")
    assert audio["preferredcodec"] == ("vorbis" if fmt == "ogg" else fmt)
    assert audio["preferredquality"] == "0"
    assert value["writethumbnail"] == (fmt not in ("wav", "ogg"))


def test_playlist_trim_normalization_and_bitrate(tmp_path):
    value = params({"playlist": "all", "outputDir": str(tmp_path), "quality": "192", "trim": {"start": 2, "end": 7}, "rateLimit": "2M", "normalize": True})
    assert "%(playlist_title|Playlist)s" in value["outtmpl"]["default"]
    assert not value["noplaylist"]
    assert value["ratelimit"] == 2 * 1024**2
    assert "download_ranges" not in value
    assert "force_keyframes_at_cuts" not in value
    audio = next(pp for pp in value["postprocessors"] if pp["key"] == "FFmpegExtractAudio")
    assert audio["trim"] == {"start": 2, "end": 7}
    assert value["postprocessor_args"]["siphonnormalize+ffmpeg_o"] == ["-af", "loudnorm=I=-16:LRA=11:TP=-1.5"]
    keys = [pp["key"] for pp in value["postprocessors"]]
    assert keys.index("SiphonNormalize") < keys.index("FFmpegMetadata") < keys.index("EmbedThumbnail")


def test_real_ytdlp_construction_accepts_all_parameters():
    # No network: constructing actual processors catches drift in yt-dlp's API.
    with create_downloader(params({"normalize": True, "embedChapters": True})) as ydl:
        assert ydl.params["js_runtimes"]["node"]["path"] == "node"
        normalizer = next(pp for pp in ydl._pps["post_process"] if pp.pp_key() == "SiphonNormalize")
        assert normalizer._configuration_args("ffmpeg", ["_o1", "_o", ""]) == ["-af", "loudnorm=I=-16:LRA=11:TP=-1.5"]


@pytest.mark.parametrize("value,expected", [(None, None), ("", None), ("2M", 2097152), ("1.5K", 1536), ("256", 256), ("1MiB", 1048576)])
def test_rate_limits(value, expected):
    assert parse_rate_limit(value) == expected


@pytest.mark.parametrize("value", ["-1M", "0", "wat", 123, "1e20", "99999999999999999999999T"])
def test_invalid_rate_limits(value):
    with pytest.raises(HostError, match="rateLimit"):
        parse_rate_limit(value)


@pytest.mark.parametrize("options", [{"format": "avi"}, {"quality": 320}, {"playlist": "no"}, {"normalize": "false"},
                                      {"trim": {"end": 2, "start": 3}}, {"trim": {"start": float("nan")}},
                                      {"outputDir": "relative"}, {"filenameTemplate": "../escape.mp3"}])
def test_invalid_options(options):
    with pytest.raises(HostError):
        validate_options(options)


def test_cookies_cleanup_even_on_exception_and_export_precedence():
    filename = None
    with pytest.raises(RuntimeError):
        with cookie_params({"cookies": "# Netscape HTTP Cookie File\n", "cookiesFromBrowser": True}) as options:
            filename = options["cookiefile"]
            assert Path(filename).exists()
            assert "cookiesfrombrowser" not in options
            raise RuntimeError("extractor failed")
    assert not Path(filename).exists()


def test_helium_cookie_profile():
    with cookie_params({"cookiesFromBrowser": True}) as options:
        browser, profile, keyring, container = options["cookiesfrombrowser"]
        assert browser == "chromium"
        assert str(Path("imput/Helium/User Data")) in profile
        assert keyring is container is None


@pytest.mark.parametrize("url", ["file:///C:/Windows/win.ini", "javascript:alert(1)", "https://", None, "https://user:secret@example.com"])
def test_invalid_urls(url):
    with pytest.raises(HostError):
        validate_url(url)
