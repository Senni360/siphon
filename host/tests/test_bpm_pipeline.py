"""Host integration tests: the numerical engine is mocked throughout."""

import json
import io
import logging
from pathlib import Path
import threading
import time
from types import SimpleNamespace
import queue

import pytest

import siphon_host.analysis as engine
import siphon_host.analysis_service as service
import siphon_host.analyze_request as requests
import siphon_host.host as host_module
import siphon_host.jobs as jobs
from siphon_host.errors import Cancelled, HostError
from siphon_host.options import validate_options
from test_host import RecordingWriter, host
from test_jobs import wait_for

RESULT = {"bpm": 120.5, "bpmConfidence": .9, "bpmCandidates": [{"bpm": 120.5, "score": 1}],
          "key": "A minor", "keyShort": "Am", "camelot": "8A", "openKey": "1m",
          "tonic": "A", "mode": "minor", "keyConfidence": .8, "durationAnalyzed": 8,
          "engine": "siphon-analysis/1"}


@pytest.fixture
def mocked(monkeypatch):
    calls = []
    monkeypatch.setattr(jobs, "analyze", lambda path, **kw: calls.append(("analyze", Path(path).name, kw)) or RESULT.copy())
    monkeypatch.setattr(jobs, "write_tags", lambda path, result: calls.append(("tags", Path(path).name, result)))
    monkeypatch.setattr(host_module, "warm_up", lambda logger: None)
    return calls


def manager_for(tmp_path, names, *, fail=False):
    events = []

    class Downloader:
        def __init__(self, params):
            self.params = params
        def __enter__(self):
            return self
        def __exit__(self, *args):
            pass
        def extract_info(self, url, download):
            for index, name in enumerate(names, 1):
                self.params["postprocessor_hooks"][0]({"postprocessor": "EmbedThumbnail", "status": "finished",
                    "info_dict": {"title": name, "playlist_index": index, "playlist_count": len(names)}})
                file = tmp_path / name
                file.write_bytes(b"completed audio")
                self.params["post_hooks"][0](str(file))
            if fail:
                raise RuntimeError("download failed after completed item")
            return {}

    manager = jobs.JobManager(events.append, logging.getLogger("test"), {"found": True, "path": None}, None, factory=Downloader)
    return manager, events


def test_pipeline_after_postprocessing_tags_before_rename_and_done(tmp_path, mocked):
    manager, events = manager_for(tmp_path, ["track.mp3"])
    try:
        job = manager.create("one", "https://example.org/a", validate_options({"outputDir": str(tmp_path), "bpmRange": [80, 160]}))
        manager._download(job, None)
        done = job.snapshot()
        assert done["state"] == "done" and done["analysis"] == RESULT and done["analysisError"] is None
        assert done["file"].endswith("track [121 BPM Am].mp3")
        assert done["files"] == [done["file"]] and Path(done["file"]).read_bytes() == b"completed audio"
        assert [call[0] for call in mocked] == ["analyze", "tags"]
        assert mocked[0][2]["bpm_range"] == (80, 160)
        assert mocked[1][1] == "track.mp3"
        states = [event["state"] for event in events]
        assert states.index("analyzing") < states.index("done")
        assert events[states.index("analyzing")]["stage"] == "analyzing"
    finally:
        manager.shutdown()


@pytest.mark.parametrize("option", ["analyze", "appendBpmKey", "writeBpmKeyTags"])
def test_disable_each_setting(tmp_path, mocked, option):
    manager, _ = manager_for(tmp_path, ["track.mp3"])
    try:
        job = manager.create("one", "https://example.org/a", validate_options({"outputDir": str(tmp_path), option: False}))
        manager._download(job, None)
        assert job.snapshot()["state"] == "done"
        assert bool(job.snapshot()["analysis"]) == (option != "analyze")
        assert any(call[0] == "tags" for call in mocked) == (option not in ("analyze", "writeBpmKeyTags"))
        assert (" BPM " in job.snapshot()["file"]) == (option not in ("analyze", "appendBpmKey"))
    finally:
        manager.shutdown()


@pytest.mark.parametrize("overwrite", [False, True])
def test_rename_collisions_and_overwrite(tmp_path, overwrite):
    source = tmp_path / "track.mp3"
    source.write_bytes(b"new")
    target = tmp_path / "track [120 BPM Am].mp3"
    target.write_bytes(b"existing")
    result = Path(service.rename_with_suffix(source, " [120 BPM Am]", overwrite=overwrite))
    assert result.name == (target.name if overwrite else "track [120 BPM Am] (2).mp3")
    assert result.read_bytes() == b"new"
    if not overwrite:
        assert target.read_bytes() == b"existing"


def test_suffix_skip(tmp_path):
    file = tmp_path / "track [120 BPM Am].mp3"
    file.write_bytes(b"audio")
    assert service.rename_with_suffix(file, " [120 BPM Am]") == str(file)
    assert len(list(tmp_path.iterdir())) == 1


def test_rename_windows_length_and_collision_budget(tmp_path):
    # Includes surrogate pairs: Windows budgets UTF-16 units, not code points.
    directory = tmp_path / ("d" * max(1, 190 - len(str(tmp_path))))
    directory.mkdir()
    file = directory / ("music" * 7 + ".mp3")
    file.write_bytes(b"audio")
    first = Path(service.rename_with_suffix(file, " [120 BPM Am]"))
    assert len(str(first).encode("utf-16-le")) // 2 <= 259
    file.write_bytes(b"second")
    second = Path(service.rename_with_suffix(file, " [120 BPM Am]"))
    assert second != first and second.name.endswith(" [120 BPM Am] (2).mp3")
    assert len(str(second).encode("utf-16-le")) // 2 <= 259


@pytest.mark.parametrize("failure", ["analysis", "tags", "rename"])
def test_failure_isolation(tmp_path, mocked, monkeypatch, failure):
    def broken(*args, **kwargs):
        raise RuntimeError("test failure")
    monkeypatch.setattr(jobs, {"analysis": "analyze", "tags": "write_tags", "rename": "rename_with_suffix"}[failure], broken)
    manager, _ = manager_for(tmp_path, ["track.mp3"])
    try:
        job = manager.create("one", "https://example.org/a", validate_options({"outputDir": str(tmp_path)}))
        manager._download(job, None)
        state = job.snapshot()
        assert state["state"] == "done" and Path(state["file"]).exists()
        assert state["analysisError"]["message"] == "test failure"
        assert (state["analysis"] is None) == (failure == "analysis")
        if failure == "tags":
            assert " BPM " in state["file"]
    finally:
        manager.shutdown()


def test_playlist_per_file_analysis_and_failed_item_keeps_next(tmp_path, mocked, monkeypatch):
    def estimate(path, **kwargs):
        if Path(path).name == "bad.mp3":
            raise ValueError("silent audio")
        return RESULT.copy()
    monkeypatch.setattr(jobs, "analyze", estimate)
    manager, _ = manager_for(tmp_path, ["first.mp3", "bad.mp3", "last.mp3"])
    try:
        job = manager.create("playlist", "https://example.org/list", validate_options({"outputDir": str(tmp_path), "playlist": "all"}))
        manager._download(job, None)
        state = job.snapshot()
        assert state["state"] == "done" and state["analysis"] == RESULT
        assert len(state["analyses"]) == len(state["files"]) == 3
        assert [row["file"] for row in state["analyses"]] == state["files"]
        assert state["analyses"][1]["analysis"] is None
        assert state["analyses"][1]["analysisError"]["message"] == "silent audio"
        assert state["analysisError"]["message"] == "silent audio"
    finally:
        manager.shutdown()


def test_cancel_during_analysis_no_tags_rename_or_done(tmp_path, mocked, monkeypatch):
    entered, release = threading.Event(), threading.Event()
    def estimate(path, **kw):
        entered.set()
        release.wait(3)
        return RESULT.copy()
    monkeypatch.setattr(engine, "analyze_file", estimate)
    monkeypatch.setattr(jobs, "analyze", service.analyze)
    manager, events = manager_for(tmp_path, ["track.mp3"])
    try:
        job = manager.create("cancel", "https://example.org/a", validate_options({"outputDir": str(tmp_path)}))
        manager.enqueue(job)
        assert entered.wait(2)
        assert manager.list_jobs()[0]["state"] == "analyzing"
        assert manager.cancel_job("cancel")
        wait_for(lambda: job.snapshot()["state"] == "cancelled", timeout=1)
        release.set()
        assert (tmp_path / "track.mp3").exists()
        assert not mocked and not any(event["state"] == "done" for event in events)
    finally:
        release.set()
        manager.shutdown()


def test_global_analysis_concurrency_and_waiting_cancellation(monkeypatch):
    entered, release, cancelled = threading.Event(), threading.Event(), threading.Event()
    lock = threading.Lock()
    counts = {"active": 0, "max": 0, "calls": 0}
    def estimate(*args, **kwargs):
        with lock:
            counts["calls"] += 1
            counts["active"] += 1
            counts["max"] = max(counts["max"], counts["active"])
            if counts["active"] == 2:
                entered.set()
        release.wait(3)
        with lock:
            counts["active"] -= 1
        return RESULT
    monkeypatch.setattr(engine, "analyze_file", estimate)
    threads = [threading.Thread(target=service.analyze, args=("mock.wav",)) for _ in range(4)]
    try:
        for thread in threads:
            thread.start()
        assert entered.wait(2)
        def check():
            raise Cancelled()
        with pytest.raises(Cancelled):
            service.analyze("cancel.wav", check_cancel=check)
        release.set()
        for thread in threads:
            thread.join(3)
        assert counts == {"active": 0, "max": 2, "calls": 4}
    finally:
        release.set()


@pytest.mark.parametrize("value", [{"bpmRange": [180, 70]}, {"bpmRange": [True, 150]}, {"bpmRange": [20, 170]},
                                   {"analyze": 1}, {"appendBpmKey": "true"}, {"writeBpmKeyTags": None},
                                   {"keyNotation": "invalid"}, {"bpmKeySuffix": "{key.foo}"}, {"bpmKeySuffix": "{bpm:.0f}"}])
def test_analysis_options_rejected(value):
    with pytest.raises(HostError) as caught:
        validate_options(value)
    assert caught.value.code == "BAD_REQUEST"


def test_local_analyze_and_input_validation(tmp_path, mocked, monkeypatch):
    file = tmp_path / "音楽.mp3"
    file.write_bytes(b"audio")
    seen = []
    monkeypatch.setattr(requests, "analyze", lambda path, **kw: seen.append((path, kw)) or RESULT)
    instance = host(RecordingWriter())
    try:
        assert instance._slow_request({"type": "analyze", "path": str(file), "bpmRange": [90, 180], "maxSeconds": 30}) == {"analysis": RESULT, "source": "file"}
        assert seen[0][1]["max_seconds"] == 30 and seen[0][1]["bpm_range"] == [90, 180]
        for payload, code in [({}, "BAD_REQUEST"), ({"path": str(file), "url": "https://example.org"}, "BAD_REQUEST"),
                              ({"path": str(tmp_path)}, "DISK"), ({"path": str(file) + ".missing"}, "DISK"),
                              ({"path": "relative.mp3"}, "BAD_REQUEST"), ({"path": str(file), "maxSeconds": 151}, "BAD_REQUEST")]:
            with pytest.raises(HostError) as caught:
                instance._slow_request({"type": "analyze", **payload})
            assert caught.value.code == code
    finally:
        instance.shutdown()


@pytest.mark.parametrize("youtube", [False, True])
@pytest.mark.parametrize("fail", [False, True])
def test_url_lowest_audio_range_cookies_fallback_cleanup(mocked, monkeypatch, youtube, fail):
    seen = []
    cookie = "# Netscape HTTP Cookie File\n"
    def estimate(path, **kwargs):
        assert path.is_file()
        if fail:
            raise RuntimeError("analysis failed")
        return RESULT
    monkeypatch.setattr(requests, "analyze", estimate)
    class Downloader:
        def __init__(self, params):
            seen.append(params)
            self.params = params
        def __enter__(self):
            return self
        def __exit__(self, *args):
            pass
        def add_post_processor(self, *args, **kwargs):
            pass
        def extract_info(self, url, download):
            assert download and Path(self.params["cookiefile"]).read_text() == cookie
            assert self.params["format"] == "worstaudio"
            assert list(self.params["download_ranges"]({"duration": 300}, self)) == [{"start_time": 0, "end_time": 150}]
            file = Path(self.params["paths"]["home"]) / "excerpt.webm"
            file.write_bytes(b"audio")
            self.params["post_hooks"][0](str(file))
            return {"title": "CC track"}
    instance = host(RecordingWriter(), factory=Downloader)
    try:
        payload = {"type": "analyze", "url": "https://www.youtube.com/watch?v=cc" if youtube else "https://example.org/audio", "cookies": cookie}
        if fail:
            with pytest.raises(RuntimeError):
                instance._slow_request(payload)
        else:
            assert instance._slow_request(payload) == {"analysis": RESULT, "source": "url", "title": "CC track"}
        assert seen
        assert not Path(seen[0]["paths"]["home"]).exists()
        assert not Path(seen[0]["cookiefile"]).exists()
        if youtube:
            assert seen[0]["extractor_args"]["youtube"]["player_client"] == ["default"]
    finally:
        instance.shutdown()


@pytest.mark.parametrize("path", [None, "C:\\音楽\\song.mp3"])
def test_pick_file_sta_filter_and_cancel(mocked, monkeypatch, path):
    seen = []
    monkeypatch.setattr(host_module.subprocess, "run", lambda args, **kw: seen.append((args, kw)) or SimpleNamespace(returncode=0, stdout=json.dumps(path), stderr=""))
    instance = host(RecordingWriter())
    try:
        assert instance._slow_request({"type": "pickFile"}) == {"path": path}
        command = seen[0][0]
        assert "-STA" in command and "Open audio file" in command[-1]
        for ext in ("mp3", "m4a", "aac", "flac", "wav", "ogg", "opus", "webm", "aiff"):
            assert "*." + ext in command[-1]
    finally:
        instance.shutdown()


@pytest.mark.parametrize("kind", ["analyze", "pickFile"])
def test_slow_analysis_and_picker_never_block_reader(mocked, monkeypatch, kind):
    entered, release = threading.Event(), threading.Event()
    writer = RecordingWriter()
    instance = host(writer)
    def slow(request):
        entered.set()
        release.wait(3)
        return {"path": None} if kind == "pickFile" else {"analysis": RESULT, "source": "file"}
    monkeypatch.setattr(instance, "_slow_request", slow)
    try:
        instance.dispatch({"id": "slow", "type": kind})
        assert entered.wait(2)
        instance.dispatch({"id": "hello", "type": "hello"})
        assert writer.messages[-1]["id"] == "hello" and writer.messages[-1]["ok"]
        release.set()
        wait_for(lambda: any(message.get("id") == "slow" for message in writer.messages))
    finally:
        release.set()
        instance.shutdown()


def test_analysis_deadline_keeps_temp_input_until_decoder_finishes(mocked, monkeypatch):
    entered, release = threading.Event(), threading.Event()
    paths = []
    monkeypatch.setattr(requests, "REQUEST_TIMEOUT", .2)
    def estimate(path, **kwargs):
        paths.append(path)
        entered.set()
        release.wait(3)
        assert path.is_file()
        return RESULT
    monkeypatch.setattr(requests, "analyze", estimate)
    class Downloader:
        def __init__(self, params): self.params = params
        def __enter__(self): return self
        def __exit__(self, *args): pass
        def extract_info(self, *args, **kwargs):
            file = Path(self.params["paths"]["home"]) / "excerpt.webm"
            file.write_bytes(b"audio")
            return {"filepath": str(file), "title": "CC audio"}
    instance = host(RecordingWriter(), factory=Downloader)
    try:
        with pytest.raises(HostError) as caught:
            instance._slow_request({"type": "analyze", "url": "https://example.org/audio"})
        assert caught.value.code == "NETWORK" and entered.is_set()
        assert paths[0].is_file()
        release.set()
        wait_for(lambda: not paths[0].parent.exists())
    finally:
        release.set()
        instance.shutdown()


def test_url_analysis_retries_youtube_client_without_video_fallback(mocked, monkeypatch):
    seen = []
    monkeypatch.setattr(requests, "analyze", lambda *args, **kw: RESULT)
    class Downloader:
        def __init__(self, params): self.params = params; seen.append(params)
        def __enter__(self): return self
        def __exit__(self, *args): pass
        def add_post_processor(self, *args, **kwargs): pass
        def extract_info(self, *args, **kwargs):
            if len(seen) == 1:
                raise RuntimeError("HTTP Error 403: Forbidden")
            file = Path(self.params["paths"]["home"]) / "excerpt.webm"
            file.write_bytes(b"audio")
            return {"filepath": str(file), "title": "CC audio"}
    instance = host(RecordingWriter(), factory=Downloader)
    try:
        assert instance._slow_request({"type": "analyze", "url": "https://youtu.be/cc"})["analysis"] == RESULT
        assert [params["extractor_args"]["youtube"]["player_client"] for params in seen] == [["default"], ["web_embedded"]]
        assert all(params["format"] == "worstaudio" and params["retries"] == 1 for params in seen)
        assert not Path(seen[0]["paths"]["home"]).exists()
    finally:
        instance.shutdown()


def test_warmup_once_uses_generated_tiny_audio_and_cleans_it(monkeypatch):
    seen, complete = [], threading.Event()
    monkeypatch.setattr(service, "_warming", False)
    def estimate(path, **kwargs):
        assert path.is_file() and kwargs == {"max_seconds": 3}
        seen.append(path)
        return RESULT
    class Logger:
        def info(self, *args): complete.set()
        def warning(self, *args, **kwargs): pytest.fail("Warm-up failed")
    monkeypatch.setattr(engine, "analyze_file", estimate)
    service.warm_up(Logger())
    service.warm_up(Logger())
    assert complete.wait(2)
    assert len(seen) == 1 and not seen[0].parent.exists()


def test_handler_reports_disk_and_missing_ffmpeg_codes(tmp_path, mocked):
    writer = RecordingWriter()
    instance = host(writer)
    try:
        instance.dispatch({"id": "missing", "type": "analyze", "path": str(tmp_path / "missing.mp3")})
        wait_for(lambda: bool(writer.messages))
        assert writer.messages[-1]["error"]["code"] == "DISK"
        instance.ffmpeg = {"found": False}
        instance.dispatch({"id": "ffmpeg", "type": "analyze", "url": "https://example.org/audio"})
        wait_for(lambda: any(m["id"] == "ffmpeg" for m in writer.messages))
        assert writer.messages[-1]["error"]["code"] == "FFMPEG_MISSING"
    finally:
        instance.shutdown()


def test_persistent_numerical_worker_reused_and_stopped(monkeypatch):
    available, processes, calls = queue.LifoQueue(), [], []
    available.put(None)
    monkeypatch.setattr(service, "_available", available)
    monkeypatch.setattr(service, "_processes", processes)
    monkeypatch.setattr(service, "_stopped", False)
    process = SimpleNamespace(stdin=io.StringIO(), stdout=io.StringIO((json.dumps({"analysis": RESULT}) + "\n") * 2), returncode=None)
    process.poll = lambda: process.returncode
    process.terminate = lambda: setattr(process, "returncode", 1)
    monkeypatch.setattr(service.subprocess, "Popen", lambda args, **kw: calls.append((args, kw)) or process)
    assert service._remote_analyze("C:\\音楽.mp3", bpm_range=(70, 180), max_seconds=150) == RESULT
    assert service._remote_analyze("C:\\other.mp3", bpm_range=(80, 160), max_seconds=None) == RESULT
    assert len(calls) == len(processes) == 1
    assert calls[0][0][-1] == "siphon_host.analysis_worker"
    lines = [json.loads(line) for line in process.stdin.getvalue().splitlines()]
    assert lines[0]["path"] == "C:\\音楽.mp3" and lines[1]["bpm_range"] == [80, 160]
    service.shutdown()
    assert process.poll() == 1


def test_numerical_worker_preserves_engine_error_codes(monkeypatch):
    available = queue.LifoQueue()
    process = SimpleNamespace(stdin=io.StringIO(), stdout=io.StringIO(json.dumps({"error": {"code": "DISK", "message": "missing audio"}}) + "\n"), poll=lambda: None)
    available.put(process)
    monkeypatch.setattr(service, "_available", available)
    with pytest.raises(HostError) as caught:
        service._remote_analyze("missing.mp3", bpm_range=(70, 180), max_seconds=None)
    assert caught.value.code == "DISK" and str(caught.value) == "missing audio"
    assert available.get_nowait() is process
