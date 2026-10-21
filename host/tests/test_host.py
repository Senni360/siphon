import io
import logging
import json
import struct
import threading
import time

import pytest

from siphon_host.framing import Writer, read_message
from siphon_host.host import Host


class RecordingWriter:
    closed = False
    def __init__(self):
        self.messages = []
    def write(self, message):
        self.messages.append(message)
    def close(self):
        self.closed = True


def host(writer, **kwargs):
    return Host(writer, logging.getLogger("test"), ffmpeg={"found": True, "path": None}, node={"name": "node", "version": "v24.0.0", "path": "node"}, **kwargs)


def test_request_contract_and_validation():
    writer = RecordingWriter()
    instance = host(writer)
    try:
        instance.dispatch({"id": "hello", "type": "hello", "maxConcurrent": 2})
        data = writer.messages[-1]["data"]
        assert data["jsRuntime"] == {"name": "node", "version": "v24.0.0"}
        assert data["hostVersion"]
        assert instance.jobs.limit == 2
        instance.dispatch({"id": "bad", "type": "hello", "maxConcurrent": 0})
        assert writer.messages[-1]["error"]["code"] == "BAD_REQUEST"
        instance.dispatch({"id": "unknown", "type": "bogus"})
        assert writer.messages[-1]["error"]["code"] == "BAD_REQUEST"
        instance.dispatch({"id": "jobs", "type": "listJobs"})
        assert writer.messages[-1]["data"] == {"jobs": []}
    finally:
        instance.shutdown()


def test_probe_is_async_and_does_not_block_hello():
    entered, release = threading.Event(), threading.Event()
    calls = []
    class FakeYDL:
        def __init__(self, params):
            calls.append(params)
        def __enter__(self):
            return self
        def __exit__(self, *args):
            pass
        def extract_info(self, url, **kwargs):
            assert kwargs == {"download": False, "process": False}
            entered.set()
            release.wait(3)
            return {"id": "1", "title": "Track", "is_live": True, "formats": [{"format_id": "audio", "acodec": "opus", "ext": "webm"}]}
    writer = RecordingWriter()
    instance = host(writer, factory=FakeYDL)
    try:
        instance.dispatch({"id": "probe", "type": "probe", "url": "https://example.org/video"})
        assert entered.wait(2)
        instance.dispatch({"id": "hello", "type": "hello"})
        assert writer.messages[-1]["id"] == "hello"
        release.set()
        deadline = time.monotonic() + 2
        while not any(m["id"] == "probe" for m in writer.messages) and time.monotonic() < deadline:
            time.sleep(.01)
        result = next(m for m in writer.messages if m["id"] == "probe")
        assert result["data"]["isLive"]
        assert result["data"]["formatsAudio"][0]["acodec"] == "opus"
        assert calls[0]["extract_flat"] == "in_playlist"
    finally:
        release.set()
        instance.shutdown()


def test_clean_eof_shutdown_is_immediate():
    writer = RecordingWriter()
    instance = host(writer)
    instance.read_loop(io.BytesIO())
    assert instance.stop.is_set()
    assert writer.closed


def frame(payload):
    return struct.pack("<I", len(payload)) + payload


@pytest.mark.parametrize("payload", [b'{"id":"bad",', b'[]', b'null', b'42', b'"text"', b'\xff', b'',
                                      b'{"id":"bad","value":NaN}', b'{"id":"\\ud800"}', b'[' * 2000])
def test_bad_frame_returns_error_and_reader_accepts_next_request(payload):
    writer = RecordingWriter()
    instance = host(writer)
    instance.read_loop(io.BytesIO(frame(payload) + frame(b'{"id":"next","type":"hello"}')))
    assert writer.messages[0]["id"] is None
    assert writer.messages[0]["type"] == "result"
    assert writer.messages[0]["error"]["code"] == "BAD_REQUEST"
    assert not writer.messages[0]["ok"]
    assert writer.messages[1]["id"] == "next" and writer.messages[1]["ok"]


@pytest.mark.parametrize("raw", [b'\x01', struct.pack('<I', 5) + b'{}', struct.pack('<I', 16 * 1024 * 1024 + 1)])
def test_unrecoverable_frames_are_logged_and_close_reader(raw, caplog):
    writer = RecordingWriter()
    instance = host(writer)
    instance.read_loop(io.BytesIO(raw))
    assert writer.closed and not writer.messages
    assert "Native messaging input closed or malformed" in caplog.text


def test_handler_exception_cannot_kill_reader(monkeypatch):
    writer = RecordingWriter()
    instance = host(writer)
    dispatch = instance.dispatch
    def failing_handler(request):
        if request["id"] == "broken":
            raise RuntimeError("Handler failed")
        return dispatch(request)
    monkeypatch.setattr(instance, "dispatch", failing_handler)
    requests = [{"id": "broken", "type": "hello"}, {"id": "next", "type": "hello"}]
    instance.read_loop(io.BytesIO(b''.join(frame(json.dumps(request).encode()) for request in requests)))
    assert writer.messages[0]["id"] == "broken" and not writer.messages[0]["ok"]
    assert writer.messages[1]["id"] == "next" and writer.messages[1]["ok"]


def test_native_file_actions_use_absolute_paths_without_shell(tmp_path, monkeypatch):
    import siphon_host.host as module
    writer = RecordingWriter()
    instance = host(writer)
    file = tmp_path / "a file with spaces.mp3"
    file.write_bytes(b"audio")
    launched, opened = [], []
    monkeypatch.setattr(module.subprocess, "Popen", lambda args, **kwargs: launched.append((args, kwargs)))
    monkeypatch.setattr(module.os, "startfile", lambda value: opened.append(value), raising=False)
    try:
        assert instance._slow_request({"type": "reveal", "path": str(file)}) == {}
        assert launched[0][0] == ["explorer.exe", "/select,", str(file)]
        assert not launched[0][1].get("shell")
        assert instance._slow_request({"type": "open", "path": str(file)}) == {}
        assert opened == [str(file)]
    finally:
        instance.shutdown()


def test_folder_picker_uses_sta_env_data_and_returns_unicode(monkeypatch):
    import siphon_host.host as module
    from types import SimpleNamespace
    writer = RecordingWriter()
    instance = host(writer)
    calls = []
    def run(args, **kwargs):
        calls.append((args, kwargs))
        return SimpleNamespace(returncode=0, stdout='"C:\\\\音楽"', stderr="")
    monkeypatch.setattr(module.subprocess, "run", run)
    try:
        initial = "C:\\Music'; Write-Output 'injected"
        assert instance._slow_request({"type": "pickFolder", "initial": initial}) == {"path": "C:\\音楽"}
        args, kwargs = calls[0]
        assert "-STA" in args
        assert initial not in args[-1]
        assert kwargs["env"]["SIPHON_FOLDER_INITIAL"] == initial
    finally:
        instance.shutdown()


def test_update_runs_in_current_interpreter_and_requires_restart(monkeypatch):
    import siphon_host.host as module
    from types import SimpleNamespace
    writer = RecordingWriter()
    instance = host(writer)
    calls = []
    monkeypatch.setattr(module.subprocess, "run", lambda args, **kwargs: calls.append((args, kwargs)) or SimpleNamespace(returncode=0, stdout="updated", stderr=""))
    try:
        result = instance._slow_request({"type": "update"})
        assert result["restartRequired"] is True
        assert result["before"] and result["after"]
        args, kwargs = calls[0]
        assert args[:6] == [module.sys.executable, "-m", "pip", "install", "-U", "yt-dlp[default]"]
        assert "yt-dlp-ejs" in args
        assert kwargs["capture_output"] and kwargs["timeout"] == 300
    finally:
        instance.shutdown()
