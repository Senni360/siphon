"""Bounded analysis work and filename handling, independent of the engine.

Waiters can cancel promptly. An already running numerical call retains its slot
until it finishes, and never writes tags or renames a cancelled job's file.
"""

import ctypes
import json
import math
import os
from pathlib import Path
import struct
import subprocess
import sys
import queue
import tempfile
import threading
import time
import wave

from .errors import HostError
from .runtime import subprocess_options

_slots = threading.BoundedSemaphore(2)
_warm_lock = threading.Lock()
_warming = False
_remote = False
_process_lock = threading.Lock()
_processes = []
_available = queue.LifoQueue()
_available.put(None)
_available.put(None)
_stopped = False


def enable_subprocesses():
    """Native entrypoint: isolate Windows numerical DLL startup from DNS/I/O.

    In-process library users and unit tests retain the ordinary public API.
    """
    global _remote
    _remote = True


def shutdown():
    global _stopped
    with _process_lock:
        _stopped = True
        for process in _processes:
            if process.poll() is None:
                process.terminate()


def _remote_analyze(path, *, bpm_range, max_seconds):
    process = _available.get()
    try:
        if process is None or process.poll() is not None:
            flags = subprocess_options()
            if os.name == "nt":
                flags["creationflags"] |= subprocess.BELOW_NORMAL_PRIORITY_CLASS
            with _process_lock:
                if _stopped:
                    raise HostError("CANCELLED", "Audio analysis stopped.")
                process = subprocess.Popen([sys.executable, "-u", "-m", "siphon_host.analysis_worker"],
                    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                    text=True, encoding="utf-8", cwd=str(Path(__file__).resolve().parents[1]), **flags)
                _processes.append(process)
        request = {"path": str(path), "bpm_range": list(bpm_range), "max_seconds": max_seconds}
        process.stdin.write(json.dumps(request, ensure_ascii=True) + "\n")
        process.stdin.flush()
        line = process.stdout.readline(65536)
        if not line:
            raise HostError("INTERNAL", "The audio analysis worker exited unexpectedly.")
        reply = json.loads(line)
        if "error" in reply:
            raise HostError(reply["error"]["code"], reply["error"]["message"])
        return reply["analysis"]
    except (OSError, ValueError) as exc:
        if process and process.poll() is None:
            process.terminate()
        raise HostError("INTERNAL", "The audio analysis worker could not return a result.") from exc
    finally:
        _available.put(process)


def analyze(path, *, bpm_range=(70, 180), max_seconds=None, check_cancel=lambda: None, deadline=None):
    def check():
        check_cancel()
        if deadline is not None and time.monotonic() >= deadline:
            raise HostError("NETWORK", "Audio analysis timed out. Try a local file or a shorter clip.")

    while True:
        check()
        if _slots.acquire(timeout=.05):
            break
    finished = threading.Event()
    result = {}

    def run():
        try:
            if _remote:
                result["analysis"] = _remote_analyze(path, bpm_range=bpm_range, max_seconds=max_seconds)
            else:
                from .analysis import analyze_file
                result["analysis"] = analyze_file(path, bpm_range=tuple(bpm_range), max_seconds=max_seconds)
        except Exception as exc:
            result["error"] = exc
        finally:
            _slots.release()
            finished.set()

    try:
        threading.Thread(target=run, name="audio-analysis", daemon=True).start()
    except BaseException:
        _slots.release()
        raise
    while not finished.wait(.05):
        check()
    check()
    if "error" in result:
        raise result["error"]
    return result["analysis"]


def _units(value):
    return len(value.encode("utf-16-le")) // 2


def _truncate(value, units):
    while value and _units(value) > units:
        value = value[:-1]
    return value.rstrip(" .")


def rename_with_suffix(path, suffix, *, overwrite=False):
    """Keep extension/suffix within Windows' 259 UTF-16-unit path budget.

    Windows rename is atomic and refuses an existing target. Retrying that
    operation also protects two jobs renaming into the same output directory.
    """
    path = Path(path).absolute()
    if not suffix or path.stem.endswith(suffix):
        return str(path)
    parent_units = _units(str(path.parent)) + 1
    limit = min(255, 259 - parent_units)
    for index in range(1, 10001):
        collision = f" ({index})" if index > 1 else ""
        remaining = limit - _units(suffix + collision + path.suffix)
        if remaining < 1:
            raise HostError("DISK", "The output directory is too long to append the BPM/key suffix.")
        stem = _truncate(path.stem, remaining) or "_"
        target = path.with_name(stem + suffix + collision + path.suffix)
        if target == path:
            return str(path)
        try:
            if overwrite:
                path.replace(target)
            else:
                # Also useful on non-Windows test platforms, where rename replaces.
                if target.exists():
                    continue
                path.rename(target)
            return str(target)
        except FileExistsError:
            continue
    raise HostError("DISK", "Too many files already use this BPM/key filename.")


def warm_up(logger):
    """Start once per host, after hello has already been answered."""
    global _warming
    with _warm_lock:
        if _warming:
            return
        _warming = True

    def run():
        start = time.monotonic()
        try:
            if os.name == "nt":
                # Lower only this thread's priority; do not slow native messaging.
                kernel = ctypes.windll.kernel32
                kernel.GetCurrentThread.restype = ctypes.c_void_p
                kernel.SetThreadPriority.argtypes = [ctypes.c_void_p, ctypes.c_int]
                kernel.SetThreadPriority(kernel.GetCurrentThread(), -1)
            with tempfile.TemporaryDirectory(prefix="siphon-warm-") as directory:
                path = Path(directory) / "warm.wav"
                rate = 22050
                samples = (int(6000 * math.sin(2 * math.pi * 220 * i / rate)
                               + (6000 if i % (rate // 2) < 100 else 0)) for i in range(3 * rate))
                with wave.open(str(path), "wb") as stream:
                    stream.setparams((1, 2, rate, 0, "NONE", "not compressed"))
                    stream.writeframes(b"".join(struct.pack("<h", value) for value in samples))
                # Run on this low-priority thread, using the same CPU budget.
                if _remote:
                    analyze(path, max_seconds=3)
                else:
                    with _slots:
                        from .analysis import analyze_file
                        analyze_file(path, max_seconds=3)
            logger.info("Analysis warm-up completed in %.3fs", time.monotonic() - start)
        except Exception:
            logger.warning("Analysis warm-up failed; downloads remain available", exc_info=True)

    threading.Thread(target=run, name="analysis-warm-up", daemon=True).start()
