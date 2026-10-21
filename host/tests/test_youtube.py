import copy
import errno
import logging
from pathlib import Path

import pytest
from yt_dlp.utils import DownloadError

from siphon_host.errors import Cancelled, HostError, error_details
from siphon_host.jobs import JobManager
from siphon_host.options import validate_options
from siphon_host.youtube import STRATEGIES, extract_youtube, is_youtube_url


URL = "https://music.youtube.com/watch?v=HOfdboHvshg"


class FakeYDL:
    def __init__(self, params, action):
        self.params, self.action, self.processors = params, action, []

    def __enter__(self):
        return self

    def __exit__(self, *args):
        pass

    def add_post_processor(self, processor, when):
        assert when == "before_dl"
        self.processors.append(processor)

    def evaluate_outtmpl(self, *args):
        return ""

    def to_console_title(self, *args):
        pass

    def extract_info(self, url, download):
        assert download and url == URL
        for processor in self.processors:
            processor.run({"id": "clip", "format_id": "251", "protocol": "https",
                           "url": "https://cdn.example/audio?c=WEB_EMBEDDED_PLAYER&pot=secret-token"})
        return self.action(self.params)


def execute(tmp_path, actions, *, extra=None, can_retry=lambda: True, check_cancel=lambda: None):
    calls = []

    def factory(params):
        calls.append(params)
        return FakeYDL(params, actions[min(len(calls) - 1, len(actions) - 1)])

    params = {"logger": logging.getLogger("youtube-test"), "paths": {"home": str(tmp_path), "temp": str(tmp_path)}}
    params.update(extra or {})
    return params, calls, lambda: extract_youtube(URL, params, factory, check_cancel=check_cancel, can_retry=can_retry)


def fail(message):
    def action(params):
        raise DownloadError(message)
    return action


@pytest.mark.parametrize("url", [URL, "https://youtu.be/HOfdboHvshg", "https://www.youtube.com/shorts/HOfdboHvshg",
                                  "https://www.youtube-nocookie.com/embed/HOfdboHvshg", "https://YOUTUBE.COM./watch?v=x"])
def test_youtube_url_routing(url):
    assert is_youtube_url(url)


@pytest.mark.parametrize("url", ["https://youtube.com.evil.example/watch?v=x", "https://notyoutube.com", "https://soundcloud.com/x", "https://[broken"])
def test_generic_urls_keep_generic_path(url):
    assert not is_youtube_url(url)


def test_default_success_does_not_request_extra_clients(tmp_path, caplog):
    caplog.set_level(logging.INFO, logger="youtube-test")
    params, calls, run = execute(tmp_path, [lambda p: {"id": "clip"}])
    assert run() == {"id": "clip"}
    assert len(calls) == 1
    assert "strategy=default succeeded" in caplog.text
    assert "client=WEB_EMBEDDED_PLAYER format=251" in caplog.text
    assert "secret-token" not in caplog.text
    assert not list(tmp_path.iterdir())


def test_403_changes_client_fresh_files_and_preserves_cookie_and_provider_options(tmp_path, caplog):
    caplog.set_level(logging.INFO, logger="youtube-test")
    cookiefile = tmp_path / "cookies.txt"
    cookiefile.write_text("private cookie data")
    provider = {"server_home": ["C:/bundled-provider/server"]}
    extra = {"cookiefile": str(cookiefile), "cookiesfrombrowser": None,
             "extractor_args": {"youtube": {"po_token": ["mweb.gvs+secret-token"], "formats": ["missing_pot", "duplicate"]},
                                "youtubepot-bgutilscript": provider}, "js_runtimes": {"node": {"path": "node.exe"}}}
    original = copy.deepcopy(extra)
    staging = []

    def rejected(params):
        directory = Path(params["paths"]["temp"])
        staging.append(directory)
        (directory / "audio.part").write_bytes(b"incompatible old format")
        raise DownloadError("HTTP Error 403: Forbidden")

    def recovered(params):
        directory = Path(params["paths"]["temp"])
        staging.append(directory)
        assert not staging[0].exists()
        assert not list(directory.iterdir())
        return {"id": "clip"}

    params, calls, run = execute(tmp_path, [rejected, recovered], extra=extra)
    assert run() == {"id": "clip"}
    assert [p["extractor_args"]["youtube"]["player_client"] for p in calls] == [["default"], ["web_embedded"]]
    for p in calls:
        assert p["cookiefile"] == str(cookiefile)
        assert p["extractor_args"]["youtubepot-bgutilscript"] == provider
        assert p["extractor_args"]["youtube"]["po_token"] == ["mweb.gvs+secret-token"]
        assert p["extractor_args"]["youtube"]["formats"] == ["duplicate"]
        assert p["js_runtimes"] == original["js_runtimes"]
        assert p["http_chunk_size"] == 10 * 1024 * 1024
        assert p["retries"] == p["fragment_retries"] == 4
        assert not p["skip_unavailable_fragments"]
    assert extra == original
    assert all(not path.exists() for path in staging)
    assert cookiefile.read_text() == "private cookie data"
    assert "strategy=embedded succeeded" in caplog.text
    assert "secret-token" not in caplog.text and "private cookie data" not in caplog.text


@pytest.mark.parametrize("message", ["HTTP Error 403: Forbidden", "Sign in to confirm you're not a bot",
                                     "Requested format is not available", "The page needs to be reloaded.",
                                     "ffmpeg exited with code 3436169992"])
def test_all_strategies_before_friendly_login_error(tmp_path, message):
    params, calls, run = execute(tmp_path, [fail(message)])
    with pytest.raises(HostError, match="bot check") as caught:
        run()
    assert caught.value.code == "LOGIN_REQUIRED"
    assert [p["extractor_args"]["youtube"]["player_client"] for p in calls] == [list(s.clients) for s in STRATEGIES]
    assert "ext=m4a" in calls[2]["format"]
    assert "m3u8" in calls[3]["format"]
    assert "format_id=18" in calls[-1]["format"]


@pytest.mark.parametrize("message,code", [
    ("This is a private video. Sign in", "LOGIN_REQUIRED"),
    ("This video is age-restricted", "AGE_RESTRICTED"),
    ("Not available in your country", "GEO_BLOCKED"),
    ("ffmpeg is not installed", "FFMPEG_MISSING"),
    ("audio conversion failed", "INTERNAL"),
])
def test_non_client_failures_are_not_retried_or_relabelled(tmp_path, message, code):
    params, calls, run = execute(tmp_path, [fail(message)])
    with pytest.raises(DownloadError) as caught:
        run()
    assert len(calls) == 1
    assert error_details(caught.value)["code"] == code


def test_timeout_remains_network_error_after_exhaustion(tmp_path):
    params, calls, run = execute(tmp_path, [fail("connection timed out")])
    with pytest.raises(DownloadError) as caught:
        run()
    assert len(calls) == len(STRATEGIES)
    assert error_details(caught.value)["code"] == "NETWORK"


def test_do_not_replay_completed_playlist_items(tmp_path):
    params, calls, run = execute(tmp_path, [fail("HTTP Error 403: Forbidden")], can_retry=lambda: False)
    with pytest.raises(DownloadError):
        run()
    assert len(calls) == 1


def test_cancel_between_clients_stops_immediately_and_cleans_attempt(tmp_path):
    cancelled = False

    def action(params):
        nonlocal cancelled
        (Path(params["paths"]["temp"]) / "audio.part").write_bytes(b"partial")
        cancelled = True
        raise DownloadError("HTTP Error 403: Forbidden")

    def check():
        if cancelled:
            raise Cancelled()

    params, calls, run = execute(tmp_path, [action], check_cancel=check)
    with pytest.raises(Cancelled):
        run()
    assert len(calls) == 1 and not list(tmp_path.iterdir())


def test_mid_transfer_retry_keeps_progress_monotonic_but_resets_for_next_item(tmp_path):
    events = []

    def report(params, downloaded, total, item="first"):
        params["progress_hooks"][0]({"status": "downloading", "downloaded_bytes": downloaded,
                                     "total_bytes": total, "info_dict": {"id": item}})

    def rejected(params):
        report(params, 80, 100)
        raise DownloadError("HTTP Error 403: Forbidden")

    def recovered(params):
        report(params, 10, 200)
        report(params, 200, 200)
        report(params, 5, 100, "second")
        return {"id": "clip"}

    params, calls, run = execute(tmp_path, [rejected, recovered], extra={"progress_hooks": [events.append]})
    run()
    assert [e["downloaded_bytes"] for e in events] == [80, 10, 200, 5]
    assert [e["total_bytes"] for e in events] == [100, 200, 200, 100]
    assert [e["_siphon_progress"] for e in events] == [.8, .8, 1, .05]


def test_supplied_cookies_used_even_when_browser_cookie_setting_is_false(tmp_path):
    calls = []
    cookiepaths = []

    def factory(params):
        calls.append(params)
        cookiepaths.append(Path(params["cookiefile"]))
        assert cookiepaths[-1].read_text().startswith("# Netscape HTTP Cookie File")

        def action(params):
            if len(calls) == 1:
                raise DownloadError("HTTP Error 403: Forbidden")
            output = tmp_path / "complete.opus"
            output.write_bytes(b"test audio")
            params["post_hooks"][0](str(output))
            return {"filepath": str(output)}

        return FakeYDL(params, action)

    manager = JobManager(lambda event: None, logging.getLogger("youtube-test"),
                         {"found": True, "path": None}, None, factory=factory, concurrency=1)
    try:
        job = manager.create("cookies", URL, validate_options({"outputDir": str(tmp_path), "cookiesFromBrowser": False}))
        manager._download(job, "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tPREF\tf6=400\n")
        assert job.snapshot()["state"] == "done"
        assert len(calls) == 2
        assert all(not path.exists() for path in cookiepaths)
        assert not list(tmp_path.glob(".siphon-*"))
    finally:
        manager.shutdown()


def test_disk_errors_never_advance_client(tmp_path):
    def disk_error(params):
        raise OSError(errno.ENOSPC, "disk full")
    params, calls, run = execute(tmp_path, [disk_error])
    with pytest.raises(OSError):
        run()
    assert len(calls) == 1


def test_finished_transfer_without_total_does_not_regress_during_recovery(tmp_path):
    events = []

    def rejected(params):
        params["progress_hooks"][0]({"status": "finished", "downloaded_bytes": 100})
        raise DownloadError("HTTP Error 403: Forbidden")

    def recovered(params):
        params["progress_hooks"][0]({"status": "downloading", "downloaded_bytes": 10, "total_bytes": 200})
        return {"id": "clip"}

    params, calls, run = execute(tmp_path, [rejected, recovered], extra={"progress_hooks": [events.append]})
    run()
    assert [e["_siphon_progress"] for e in events] == [1, 1]
