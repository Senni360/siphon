"""Run with python -m siphon_host; no unframed output on stdout or stderr."""

import os
import sys
import threading

from .framing import Writer
from .runtime import LogStream, configure_logging


def main():
    if sys.platform == "win32":
        import msvcrt
        msvcrt.setmode(sys.stdin.fileno(), os.O_BINARY)
        msvcrt.setmode(sys.stdout.fileno(), os.O_BINARY)
    protocol_input, protocol_output = sys.stdin.buffer, sys.stdout.buffer
    logger = configure_logging()
    sys.stdout = LogStream(logger)
    sys.stderr = LogStream(logger)
    try:
        # Import after redirecting to catch diagnostics from transitive deps too.
        from .host import Host
        from .analysis_service import enable_subprocesses
        enable_subprocesses()
        host = Host(Writer(protocol_output), logger)
        reader = threading.Thread(target=host.read_loop, args=(protocol_input,), name="native-reader", daemon=True)
        reader.start()
        reader.join()
        return 0
    except Exception:
        logger.exception("Native host stopped unexpectedly")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
