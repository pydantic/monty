from __future__ import annotations

import asyncio
import subprocess
import sys

import pytest

from pydantic_monty import AsyncMonty


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


def test_async_call_from_later_shutdown_callback_fails_without_hanging():
    result = subprocess.run(
        [
            sys.executable,
            '-X',
            'faulthandler',
            '-c',
            """
import atexit
import asyncio

async def run():
    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            assert await session.feed_run('1 + 1') == 2

def late_callback():
    try:
        asyncio.run(run())
    except RuntimeError as error:
        assert 'Monty async runtime is shutting down' in str(error)
        print('rejected', flush=True)

# Runs after the extension's drain hook because atexit uses reverse order.
atexit.register(late_callback)
from pydantic_monty import AsyncMonty
asyncio.run(run())
""",
        ],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == 'rejected\n', result.stderr


@pytest.mark.parametrize('deferred', [False, True])
def test_shutdown_cancels_pending_async_operation(deferred: bool):
    result = subprocess.run(
        [
            sys.executable,
            '-X',
            'faulthandler',
            '-c',
            """
import atexit
import asyncio
import sys
import weakref

# This observer runs after the extension's drain and before Python finalization.
atexit.register(lambda: print('released' if reference() is None else 'retained', flush=True))
from pydantic_monty import AsyncMonty, ClassInstance

class Sink:
    def __call__(self, stream, text):
        pass

    def __del__(self):
        try:
            AsyncMonty().__aenter__()
        except RuntimeError as error:
            assert 'Monty async runtime is shutting down' in str(error)
            print('reentrant rejected', flush=True)

loop = asyncio.new_event_loop()
asyncio.set_event_loop(loop)
started = asyncio.Event()
expected = 2 if sys.argv[1] == 'True' else 1
calls = 0

async def wait_forever():
    global calls
    calls += 1
    if calls == expected:
        started.set()
    await asyncio.Event().wait()

async def setup():
    pool = AsyncMonty()
    await pool.__aenter__()
    session = await pool.checkout().__aenter__()
    sink = Sink()
    reference = weakref.ref(sink)
    operation = asyncio.ensure_future(session.feed_run(
        'import asyncio; await asyncio.gather(wait_forever(), wait_forever())' if expected == 2 else 'await wait_forever()',
        inputs={'value': ClassInstance(sink)},
        external_lookup={'wait_forever': wait_forever},
        print_callback=sink,
    ))
    del sink
    await started.wait()
    assert reference() is not None
    return pool, operation, reference

# Leave the operation pending with the loop stopped. Shutdown must cancel Rust
# bridge tasks rather than waiting for this Python callback to make progress.
pool, operation, reference = loop.run_until_complete(setup())
print('pending', flush=True)
""",
            str(deferred),
        ],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == 'pending\nreentrant rejected\nreleased\n', result.stderr


def test_shutdown_racing_with_future_creation_rejects_the_call():
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
import time

original_register = atexit.register
hooks = []
def register(function, *args, **kwargs):
    if type(function).__name__ == 'ShutdownHook':
        hooks.append(function)
    return original_register(function, *args, **kwargs)
atexit.register = register
from pydantic_monty import AsyncMonty
atexit.register = original_register
assert len(hooks) == 1

entered = threading.Event()
release = threading.Event()
outcome = []

def create_bridge():
    loop = asyncio.new_event_loop()
    original_create = loop.create_future
    def paused_create():
        entered.set()
        assert release.wait(10)
        return original_create()
    loop.create_future = paused_create
    async def run():
        try:
            AsyncMonty().__aenter__()
        except RuntimeError as error:
            outcome.append(str(error))
        else:
            outcome.append('returned an unresolved future')
    try:
        loop.run_until_complete(run())
    finally:
        loop.close()

creator = threading.Thread(target=create_bridge)
creator.start()
assert entered.wait(10)
drainer = threading.Thread(target=hooks[0])
drainer.start()

async def wait_until_closed():
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        try:
            future = AsyncMonty().__aenter__()
        except RuntimeError as error:
            assert 'Monty async runtime is shutting down' in str(error)
            return
        future.cancel()
        await asyncio.sleep(0)
    raise AssertionError('shutdown did not close admission')

try:
    # Observe closed admission before allowing the paused call to reach spawn.
    asyncio.run(wait_until_closed())
finally:
    release.set()
creator.join(10)
drainer.join(10)
assert not creator.is_alive() and not drainer.is_alive()
assert outcome == ['Monty async runtime is shutting down'], outcome
print('rejected', flush=True)
""",
        ],
        capture_output=True,
        text=True,
        timeout=40,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == 'rejected\n', result.stderr


def test_first_import_from_exit_handler_drains_pending_callback():
    result = subprocess.run(
        [
            sys.executable,
            '-X',
            'faulthandler',
            '-c',
            """
import atexit
import sys

@atexit.register
def late_import():
    global pool, operation, loop
    import asyncio
    from pydantic_monty import AsyncMonty

    class Sink:
        def __call__(self, stream, text):
            pass
        def __del__(self):
            print('released' if not sys.is_finalizing() else 'released too late', flush=True)

    loop = asyncio.new_event_loop()
    asyncio.set_event_loop(loop)
    started = asyncio.Event()
    async def wait_forever():
        started.set()
        await asyncio.Event().wait()
    async def setup():
        pool = AsyncMonty()
        await pool.__aenter__()
        session = await pool.checkout().__aenter__()
        operation = asyncio.ensure_future(session.feed_run(
            'await wait_forever()',
            external_lookup={'wait_forever': wait_forever},
            print_callback=Sink(),
        ))
        await started.wait()
        return pool, operation
    pool, operation = loop.run_until_complete(setup())
    print('late import pending', flush=True)
""",
        ],
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == 'late import pending\nreleased\n', result.stderr
    assert 'PanicException' not in result.stderr


@pytest.mark.skipif(sys.platform == 'win32', reason='os.fork is unavailable on Windows')
@pytest.mark.parametrize('paused_creation', [True, False])
def test_fork_child_rejects_async_reuse_and_exits(paused_creation: bool):
    result = subprocess.run(
        [
            sys.executable,
            '-X',
            'faulthandler',
            '-c',
            """
import asyncio
import os
import signal
import sys
import threading
import time
from pydantic_monty import AsyncMonty

paused = sys.argv[1] == 'True'
entered = threading.Event()
release = threading.Event()

def create_bridge():
    loop = asyncio.new_event_loop()
    original_create = loop.create_future
    def create_future():
        entered.set()
        assert release.wait(10)
        return original_create()
    loop.create_future = create_future
    async def run():
        AsyncMonty().__aenter__()
    try:
        loop.run_until_complete(run())
    finally:
        loop.close()

async def pending_callback():
    entered.set()
    await asyncio.Event().wait()

async def setup():
    pool = AsyncMonty()
    await pool.__aenter__()
    session = await pool.checkout().__aenter__()
    operation = asyncio.ensure_future(session.feed_run(
        'await pending_callback()', external_lookup={'pending_callback': pending_callback},
    ))
    while not entered.is_set():
        await asyncio.sleep(0)
    return pool, session, operation

if paused:
    creator = threading.Thread(target=create_bridge)
    creator.start()
    assert entered.wait(10)
else:
    loop = asyncio.new_event_loop()
    asyncio.set_event_loop(loop)
    pool, session, operation = loop.run_until_complete(setup())

pid = os.fork()
if pid == 0:
    async def try_reuse():
        try:
            AsyncMonty().__aenter__()
        except RuntimeError as error:
            assert 'cannot be reused after fork' in str(error)
            if not paused:
                try:
                    pool.__aexit__(None, None, None)
                except RuntimeError as close_error:
                    assert 'cannot be reused after fork' in str(close_error)
                else:
                    raise AssertionError('fork child closed inherited pool')
            print('child rejected', flush=True)
        else:
            raise AssertionError('fork child admitted async work')
    asyncio.run(try_reuse())
    if not paused:
        operation.cancel()
        while loop._ready:
            loop._ready.popleft()._run()
    # Use normal exit so inherited atexit hooks actually run.
    sys.exit(0)

try:
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        waited, status = os.waitpid(pid, os.WNOHANG)
        if waited:
            assert os.waitstatus_to_exitcode(status) == 0, status
            break
        time.sleep(0.01)
    else:
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
        raise AssertionError('fork child hung while exiting')
finally:
    if paused:
        release.set()
        creator.join(10)
        assert not creator.is_alive()
print('parent finished', flush=True)
""",
            str(paused_creation),
        ],
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == 'child rejected\nparent finished\n', result.stderr
    assert 'PanicException' not in result.stderr
    assert 'Exception in callback' not in result.stderr


async def test_sync_callback_can_return_nested_async_future():
    async with AsyncMonty(max_processes=2) as pool:
        async with pool.checkout() as outer, pool.checkout() as inner:

            async def await_future(future):
                assert isinstance(future, asyncio.Future)
                return await future

            def nested():
                return await_future(inner.feed_run('42'))

            result = await outer.feed_run('await nested()', external_lookup={'nested': nested})
            assert result == 42
