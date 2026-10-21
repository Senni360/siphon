import logging
from pathlib import Path
import threading
import time

import pytest

from siphon_host.errors import Cancelled
from siphon_host.jobs import Job, JobManager
from siphon_host.options import validate_options


def wait_for(predicate, timeout=3):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(.01)
    assert predicate(), "Timed out waiting for worker"


def test_progress_fields_playlist_and_cooperative_cancel(tmp_path):
    events = []
    job = Job("j", "https://example.org", {}, events.append)
    partial = tmp_path / "song.webm.part"
    partial.write_bytes(b"partial")
    try:
        job.progress_hook({"status": "downloading", "downloaded_bytes": 50, "total_bytes": 100,
                           "speed": 20, "eta": 2, "tmpfilename": str(partial),
                           "info_dict": {"title": "Song", "playlist_index": 2, "playlist_count": 3}})
        assert events[-1]["progress"] == .5
        assert events[-1]["item"] == {"index": 2, "count": 3, "title": "Song"}
        job.cancel.set()
        with pytest.raises(Cancelled):
            job.progress_hook({})
        job.cleanup_parts()
        assert not partial.exists()
    finally:
        job.dispose()


def test_progress_is_coalesced_at_five_per_second_and_terminal_is_immediate():
    events = []
    job = Job("j", "https://example.org", {}, lambda data: events.append((time.monotonic(), data)))
    try:
        for index in range(60):
            job.update(state="downloading", progress=index / 60)
            time.sleep(.005)
        time.sleep(.15)
        job.update(state="done", stage="done", progress=1)
        normal = [(stamp, event) for stamp, event in events if event["state"] != "done"]
        assert len(normal) <= 3
        assert all(b[0] - a[0] >= .19 for a, b in zip(normal, normal[1:]))
        assert events[-1][1]["state"] == "done"
        count = len(events)
        job.update(state="downloading")
        time.sleep(.21)
        assert len(events) == count
    finally:
        job.dispose()


def test_queue_running_cancel_and_final_postprocessed_paths(tmp_path):
    entered, release = threading.Event(), threading.Event()
    emitted, calls = [], []

    class FakeYDL:
        def __init__(self, params):
            self.params = params
        def __enter__(self):
            return self
        def __exit__(self, *args):
            pass
        def extract_info(self, url, download):
            calls.append(url)
            entered.set()
            release.wait(3)
            self.params["progress_hooks"][0]({"status": "finished", "info_dict": {"title": "Track"}})
            self.params["post_hooks"][0](str(tmp_path / "final.mp3"))
            return {"filepath": str(tmp_path / "stale.webm")}

    manager = JobManager(emitted.append, logging.getLogger("test"), {"found": True, "path": None}, None, factory=FakeYDL, concurrency=1)
    options = validate_options({"outputDir": str(tmp_path), "sponsorblockRemove": False})
    try:
        first = manager.create("first", "https://example.org/1", options)
        second = manager.create("second", "https://example.org/2", options)
        third = manager.create("third", "https://example.org/3", options)
        manager.enqueue(first)
        assert entered.wait(2)
        manager.enqueue(second)
        manager.enqueue(third)
        assert second.snapshot()["state"] == "queued"
        assert manager.cancel_job("second")
        assert second.snapshot()["state"] == "cancelled"
        assert manager.cancel_job("first")
        release.set()
        wait_for(lambda: third.snapshot()["state"] == "done")
        assert first.snapshot()["state"] == "cancelled"
        assert "https://example.org/2" not in calls
        assert third.snapshot()["files"] == [str(tmp_path / "final.mp3")]
        assert third.snapshot()["file"].endswith("final.mp3")
        assert manager.list_jobs() == []
    finally:
        release.set()
        manager.shutdown()


def test_live_filter_rejects_stream_and_probe_state():
    from siphon_host.errors import HostError
    job = Job("j", "https://example.org", {}, lambda event: None)
    try:
        with pytest.raises(HostError) as caught:
            job.match_filter({"is_live": True}, incomplete=False)
        assert caught.value.code == "LIVE_NOT_SUPPORTED"
    finally:
        job.dispose()


@pytest.mark.parametrize("cancel", [False, True])
def test_error_and_cancel_clean_exclusive_staging_keep_completed_and_unrelated_files(tmp_path, cancel):
    completed = tmp_path / "finished-playlist-item.mp3"
    completed.write_bytes(b"completed audio")
    unrelated = tmp_path / "another-job.part"
    unrelated.write_bytes(b"another job is downloading")
    staging_paths = []
    class FailingYDL:
        def __init__(self, params):
            self.params = params
        def __enter__(self):
            return self
        def __exit__(self, *args):
            pass
        def extract_info(self, url, download):
            staging = Path(self.params["paths"]["temp"])
            staging_paths.append(staging)
            for name in ("song.webm.part", "song.webm.part-Frag1", "song.ytdl", "song.temp.mp3", "song.jpg", "song.webp"):
                (staging / name).write_bytes(b"unfinished")
            self.params["post_hooks"][0](str(completed))
            if cancel:
                job.cancel.set()
            raise RuntimeError("Synthetic conversion failure")
    manager = JobManager(lambda data: None, logging.getLogger("test"), {"found": True, "path": None}, None, factory=FailingYDL, concurrency=1)
    try:
        job = manager.create("failure", "https://example.org/audio", validate_options({"outputDir": str(tmp_path)}))
        manager._download(job, None)
        assert job.snapshot()["state"] == ("cancelled" if cancel else "error")
        assert staging_paths and not staging_paths[0].exists()
        assert completed.read_bytes() == b"completed audio"
        assert unrelated.read_bytes() == b"another job is downloading"
    finally:
        manager.shutdown()


@pytest.mark.parametrize("failure,bytes_received,expected_calls", [
    ("HTTP Error 403: Forbidden", 0, 2),
    ("HTTP Error 403: Forbidden", 1, 1),
    ("HTTP Error 500: Server Error", 0, 1),
])
def test_refresh_rejected_media_url_once_only_before_transfer(tmp_path, failure, bytes_received, expected_calls):
    calls = []
    class RetryYDL:
        def __init__(self, params):
            self.params = params
        def __enter__(self):
            return self
        def __exit__(self, *args):
            pass
        def extract_info(self, url, download):
            calls.append(url)
            if len(calls) == 1:
                job.update(downloadedBytes=bytes_received)
                raise RuntimeError(failure)
            output = tmp_path / "complete.mp3"
            output.write_bytes(b"generated audio")
            self.params["post_hooks"][0](str(output))
            return {"filepath": str(output)}
    manager = JobManager(lambda data: None, logging.getLogger("test"), {"found": True, "path": None}, None, factory=RetryYDL, concurrency=1)
    try:
        job = manager.create("retry", "https://example.org/audio", validate_options({"outputDir": str(tmp_path)}))
        manager._download(job, None)
        assert len(calls) == expected_calls
        assert job.snapshot()["state"] == ("done" if expected_calls == 2 else "error")
        assert not list(tmp_path.glob(".siphon-*"))
    finally:
        manager.shutdown()
