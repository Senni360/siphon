import io
import json
import struct
from concurrent.futures import ThreadPoolExecutor

import pytest

from siphon_host.framing import FrameError, MAX_MESSAGE_BYTES, MAX_REQUEST_BYTES, Writer, read_message


def test_utf8_roundtrip_and_clean_eof():
    stream = io.BytesIO()
    Writer(stream).write({"title": "Björk 音楽", "ok": True})
    stream.seek(0)
    assert read_message(stream) == {"title": "Björk 音楽", "ok": True}
    assert read_message(stream) is None


class ShortReads(io.BytesIO):
    def read(self, size=-1):
        return super().read(min(size, 2))


def test_partial_reads():
    raw = json.dumps({"id": "abc"}).encode()
    assert read_message(ShortReads(struct.pack("<I", len(raw)) + raw)) == {"id": "abc"}


@pytest.mark.parametrize("raw", [b"\x01", struct.pack("<I", 5) + b"{}", struct.pack("<I", 0),
                                  struct.pack("<I", MAX_REQUEST_BYTES + 1), struct.pack("<I", 1) + b"[",
                                  struct.pack("<I", 2) + b"[]", struct.pack("<I", 1) + b"\xff"])
def test_malformed_frames(raw):
    with pytest.raises(FrameError):
        read_message(io.BytesIO(raw))


def test_concurrent_writes_are_complete_frames():
    stream = io.BytesIO()
    writer = Writer(stream)
    with ThreadPoolExecutor(max_workers=10) as pool:
        list(pool.map(lambda i: writer.write({"id": i, "text": "ü" * 100}), range(100)))
    stream.seek(0)
    messages = [read_message(stream) for _ in range(100)]
    assert {m["id"] for m in messages} == set(range(100))
    assert read_message(stream) is None


def test_oversize_response_rejected_before_write():
    stream = io.BytesIO()
    with pytest.raises(FrameError):
        Writer(stream).write({"data": "x" * MAX_MESSAGE_BYTES})
    assert stream.getvalue() == b""


def test_broken_pipe_marks_writer_closed():
    class Broken:
        def write(self, data):
            raise BrokenPipeError()
    writer = Writer(Broken())
    writer.write({})
    assert writer.closed
