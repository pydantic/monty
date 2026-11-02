from __future__ import annotations

import subprocess
import sys


def test_async_result_delivery_finishes_before_interpreter_shutdown():
    result = subprocess.run(
        [
            sys.executable,
            '-X',
            'faulthandler',
            '-c',
            """
import atexit
import asyncio
import threading

pending = 0
overlapped_shutdown = False
release = threading.Event()
# Registered before the extension's drain hook, so this checks after it runs.
atexit.register(lambda: print('drained' if overlapped_shutdown and pending == 0 else 'pending'))

from pydantic_monty import AsyncMonty

async def main():
    loop = asyncio.get_running_loop()
    original_write = loop._write_to_self
    main_thread = threading.get_ident()

    def delayed_write():
        global pending
        if threading.get_ident() == main_thread:
            return original_write()
        pending += 1
        try:
            original_write()
            # The loop can finish while Rust still owns this delivery's Python objects.
            release.wait()
        finally:
            pending -= 1

    loop._write_to_self = delayed_write
    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            assert await session.feed_run('1 + 1') == 2

asyncio.run(main())

def release_deliveries():
    global overlapped_shutdown
    overlapped_shutdown = pending > 0
    release.set()

# Runs before the extension's drain hook, after confirming a delivery is blocked.
atexit.register(release_deliveries)
""",
        ],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == 'drained\n', result.stderr
