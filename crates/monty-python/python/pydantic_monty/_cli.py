"""Runs the `monty` binary, forwarding this process's arguments to it.

Reached two ways: `python -m pydantic_monty` (via `__main__.py`) and the
`pydantic-monty` console script the metapackage declares. Both exist because the
binary ships in `pydantic-monty-runtime`, so neither of the other two
distributions provides an executable of its own; the console script deliberately
is NOT named `monty`, since that would collide with — and silently overwrite —
the real binary in the scripts directory, leaving [`find_monty_binary`]
resolving to the shim itself.
"""

from __future__ import annotations

import os
import subprocess
import sys

from ._binary import find_monty_binary


def main() -> int:
    """Hands this process's arguments, streams and exit status to the `monty` binary."""
    # find_monty_binary may raise FileNotFoundError, which we let propagate to the caller
    binary = find_monty_binary()
    argv = [binary, *sys.argv[1:]]
    if os.name == 'posix':
        # can raise OSError, which we let propagate to the caller
        # os.execv never returns, so signals and the exit status are the binary's own
        os.execv(binary, argv)
    else:
        # Windows has no `exec`, so wait on a child instead. Ctrl-C reaches the
        # whole console group, so ignore it here and let the binary decide whether
        # a SIGINT ends the session — exiting early would orphan it on the terminal.
        proc = subprocess.Popen(argv)
        while True:
            try:
                return proc.wait()
            except KeyboardInterrupt:
                pass
