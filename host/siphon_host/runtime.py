"""Machine discovery and file-only diagnostics."""

import glob
import logging
from logging.handlers import RotatingFileHandler
import os
from pathlib import Path
import shutil
import subprocess
import sys


def local_app_data() -> Path:
    return Path(os.environ.get("LOCALAPPDATA", str(Path.home() / "AppData" / "Local")))


def default_output_dir() -> str:
    return str(Path(os.environ.get("USERPROFILE", str(Path.home()))) / "Music" / "Siphon")


def tray_pipe_name() -> str:
    """Per-user local-socket name of the drag tray (a named pipe on Windows)."""
    import getpass
    return "siphon-dropzone-" + "".join(c for c in getpass.getuser() if c.isalnum())


def subprocess_options() -> dict:
    return {"creationflags": subprocess.CREATE_NO_WINDOW} if sys.platform == "win32" else {}


def configure_logging() -> logging.Logger:
    logger = logging.getLogger("siphon")
    logger.setLevel(logging.DEBUG)
    logger.propagate = False
    directory = local_app_data() / "Siphon" / "logs"
    directory.mkdir(parents=True, exist_ok=True)
    handler = RotatingFileHandler(directory / "host.log", maxBytes=2_000_000, backupCount=3, encoding="utf-8")
    handler.setFormatter(logging.Formatter("%(asctime)s %(levelname)s %(threadName)s %(message)s"))
    logger.addHandler(handler)
    return logger


def executable_version(path: str, flag: str = "--version") -> str | None:
    try:
        result = subprocess.run([path, flag], capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=8, **subprocess_options())
        return result.stdout.splitlines()[0][:250] if result.returncode == 0 and result.stdout else None
    except (OSError, subprocess.TimeoutExpired):
        return None


def discover_ffmpeg() -> dict:
    path = shutil.which("ffmpeg")
    if not path:
        base = local_app_data()
        matches = glob.glob(str(base / "Microsoft" / "WinGet" / "Packages" / "Gyan.FFmpeg*" / "**" / "bin" / "ffmpeg.exe"), recursive=True)
        fallback = base / "Siphon" / "ffmpeg" / "bin" / "ffmpeg.exe"
        path = next(iter(sorted(matches, reverse=True)), None) or (str(fallback) if fallback.is_file() else None)
    version = executable_version(path, "-version") if path else None
    return {"found": bool(path and version), "version": version, "path": path}


def discover_node() -> dict | None:
    path = shutil.which("node")
    if not path:
        return None
    version = executable_version(path)
    return {"name": "node", "version": version, "path": path} if version else None


class YTDLPLogger:
    """yt-dlp never writes its console output into the framed transport."""
    def __init__(self, logger: logging.Logger, event=None):
        self.logger, self.event = logger, event

    def debug(self, message):
        self.logger.debug("%s", message)

    def info(self, message):
        self.logger.info("%s", message)

    def warning(self, message):
        self.logger.warning("%s", message)
        if self.event:
            self.event("warn", str(message))

    def error(self, message):
        self.logger.error("%s", message)
        if self.event:
            self.event("error", str(message))


class LogStream:
    """Catch incidental prints from dependencies without touching protocol output."""
    encoding = "utf-8"

    def __init__(self, logger):
        self.logger = logger

    def write(self, value):
        if value.strip():
            self.logger.debug("%s", value.rstrip())
        return len(value)

    def flush(self):
        pass

    def isatty(self):
        return False
