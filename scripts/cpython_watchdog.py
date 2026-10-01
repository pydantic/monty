"""Interrupt support for the monty-datatest CPython watchdog.

The Rust harness calls `interrupt_thread` from a helper thread once the CPython
side of a fixture has run past its deadline; see `CpythonWatchdog` in
`crates/monty-datatest/src/main.rs`.
"""

from __future__ import annotations

import ctypes


class CPythonTestTimeout(BaseException):
    """Raised asynchronously in a test thread that exceeded the watchdog deadline.

    A `BaseException` so a fixture's own `except Exception` cannot swallow it.
    """

    def __str__(self) -> str:
        return 'CPython side of the test exceeded the monty-datatest watchdog deadline'


def interrupt_thread(thread_ident: int) -> bool:
    """Raise `CPythonTestTimeout` in thread `thread_ident` at its next bytecode boundary.

    Returns whether a matching thread state was found. A thread stuck in C code
    that never returns to the eval loop cannot be interrupted this way; the Rust
    watchdog then exits the process instead.
    """
    found = ctypes.pythonapi.PyThreadState_SetAsyncExc(
        ctypes.c_ulong(thread_ident), ctypes.py_object(CPythonTestTimeout)
    )
    return found == 1


def clear_pending(thread_ident: int) -> None:
    """Discard an interrupt raised in thread `thread_ident` that it has not delivered yet."""
    ctypes.pythonapi.PyThreadState_SetAsyncExc(ctypes.c_ulong(thread_ident), None)
