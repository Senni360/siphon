"""Bounded Chrome native messaging frames; stdout is exclusively this transport."""

import json
import struct
import threading
from typing import BinaryIO

MAX_MESSAGE_BYTES = 1024 * 1024
MAX_REQUEST_BYTES = 16 * 1024 * 1024


class FrameError(ValueError):
    pass


class BadRequestFrame(FrameError):
    """The complete frame was consumed; the next header is safe to read."""


def _read_exact(stream: BinaryIO, size: int) -> bytes:
    chunks = bytearray()
    while len(chunks) < size:
        chunk = stream.read(size - len(chunks))
        if not chunk:
            raise FrameError("Truncated native messaging frame")
        chunks.extend(chunk)
    return bytes(chunks)


def read_message(stream: BinaryIO) -> dict | None:
    header = stream.read(4)
    if not header:
        return None
    if len(header) < 4:
        header += _read_exact(stream, 4 - len(header))
    size, = struct.unpack("<I", header)
    if size > MAX_REQUEST_BYTES:
        raise FrameError("Native messaging request exceeds 16 MiB; cannot resynchronize")
    payload = _read_exact(stream, size)
    if not payload:
        raise BadRequestFrame("Native messaging request cannot be empty")
    try:
        def invalid_constant(value):
            raise ValueError(f"Invalid JSON constant: {value}")
        message = json.loads(payload.decode("utf-8"), parse_constant=invalid_constant)
        # Escaped lone surrogates are accepted by json.loads but cannot be sent
        # back through our UTF-8 transport (including in a request id).
        json.dumps(message, ensure_ascii=False).encode("utf-8")
    except (UnicodeError, ValueError, RecursionError) as exc:
        raise BadRequestFrame("Invalid UTF-8 JSON request") from exc
    if not isinstance(message, dict):
        raise BadRequestFrame("Native messaging request must be an object")
    return message


class Writer:
    def __init__(self, stream: BinaryIO):
        self.stream = stream
        self.lock = threading.Lock()
        self.closed = False

    def write(self, message: dict) -> None:
        payload = json.dumps(message, ensure_ascii=False, allow_nan=False, separators=(",", ":")).encode("utf-8")
        if len(payload) > MAX_MESSAGE_BYTES:
            raise FrameError("Native messaging response exceeds 1 MiB")
        with self.lock:
            if self.closed:
                return
            try:
                self.stream.write(struct.pack("<I", len(payload)) + payload)
                self.stream.flush()
            except (BrokenPipeError, OSError, ValueError):
                self.closed = True

    def close(self) -> None:
        with self.lock:
            self.closed = True
