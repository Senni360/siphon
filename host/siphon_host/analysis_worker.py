"""Private persistent numerical worker; never speaks to the browser directly.

Load numerical DLLs on this process's main thread, without simultaneous native
pipe/DNS workers. The host owns at most two of these below-normal-priority
processes, and warms one after hello. Audio remains on this machine.
"""

import json
import os
import sys

from .analysis import analyze_file
from .errors import error_details


def main():
    # Avoid BLAS multiplying the two-worker CPU budget into dozens of threads.
    os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")
    os.environ.setdefault("OMP_NUM_THREADS", "1")
    output = sys.stdout
    sys.stdout = sys.stderr  # dependency diagnostics cannot corrupt replies
    for line in sys.stdin:
        try:
            request = json.loads(line)
            result = analyze_file(request["path"], bpm_range=tuple(request["bpm_range"]), max_seconds=request["max_seconds"])
            response = {"analysis": result}
        except Exception as exc:
            response = {"error": error_details(exc)}
        output.write(json.dumps(response, ensure_ascii=True) + "\n")
        output.flush()


if __name__ == "__main__":
    main()
