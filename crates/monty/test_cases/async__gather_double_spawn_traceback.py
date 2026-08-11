# run-async
"""Locks in the exception type, message, and frame layout for the canonical
cross-gather coroutine reuse case (the reachable-on-live-server repro from
`01-gather-double-spawn.md`)."""

import asyncio


async def foo():
    return [1, 2, 3]


c = foo()
g1 = asyncio.gather(c)
g2 = asyncio.gather(c)
await asyncio.gather(g1, g2)  # pyright: ignore
"""
TRACEBACK:
Traceback (most recent call last):
  File "async__gather_double_spawn_traceback.py", line 16, in <module>
    await asyncio.gather(g1, g2)  # pyright: ignore
    ~~~~~~~~~~~~~~~~~~~~~~~~~~~~
RuntimeError: cannot reuse already awaited coroutine
"""
