"""Tests for the suspendable `feed_start` surface and `load_snapshot`."""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest
from inline_snapshot import snapshot

from pydantic_monty import (
    AsyncFunctionSnapshot,
    AsyncFutureSnapshot,
    AsyncMonty,
    FunctionSnapshot,
    FutureSnapshot,
    Monty,
    MontyComplete,
    MontyRuntimeError,
    MontySession,
    MountDir,
    NameLookupSnapshot,
    OsFunction,
)


def test_function_call_suspends_then_completes(session: MontySession):
    snap = session.feed_start('x = add(2, 3)\nx * 10')
    assert isinstance(snap, FunctionSnapshot)
    assert snap.function_name == snapshot('add')
    assert snap.args == snapshot((2, 3))
    assert snap.kwargs == snapshot({})
    assert snap.is_os_function == snapshot(False)
    assert snap.is_method_call == snapshot(False)
    done = snap.resume({'return_value': 5})
    assert isinstance(done, MontyComplete)
    assert done.output == snapshot(50)


def test_kwargs_surface(session: MontySession):
    snap = session.feed_start('greet(name="ada", times=2)')
    assert isinstance(snap, FunctionSnapshot)
    assert snap.kwargs == snapshot({'name': 'ada', 'times': 2})


def test_resume_with_exception_instance(session: MontySession):
    snap = session.feed_start('r = None\ntry:\n    boom()\nexcept ValueError as e:\n    r = str(e)\nr')
    assert isinstance(snap, FunctionSnapshot)
    done = snap.resume({'exception': ValueError('nope')})
    assert isinstance(done, MontyComplete)
    assert done.output == snapshot('nope')


def test_resume_with_exc_type(session: MontySession):
    snap = session.feed_start('r = None\ntry:\n    boom()\nexcept ValueError as e:\n    r = str(e)\nr')
    assert isinstance(snap, FunctionSnapshot)
    done = snap.resume({'exc_type': 'ValueError', 'message': 'bad'})
    assert isinstance(done, MontyComplete)
    assert done.output == snapshot('bad')


def test_name_lookup_resume_with_value(session: MontySession):
    snap = session.feed_start('missing + 1')
    assert isinstance(snap, NameLookupSnapshot)
    assert snap.variable_name == snapshot('missing')
    done = snap.resume(value=41)
    assert isinstance(done, MontyComplete)
    assert done.output == snapshot(42)


def test_name_lookup_resume_with_none_value(session: MontySession):
    # an explicit None binds the name to None — distinct from omitting value,
    # which raises NameError
    snap = session.feed_start('x = missing\nx is None')
    assert isinstance(snap, NameLookupSnapshot)
    done = snap.resume(value=None)
    assert isinstance(done, MontyComplete)
    assert done.output == snapshot(True)


def test_name_lookup_resume_without_value_raises_name_error(session: MontySession):
    snap = session.feed_start('missing + 1')
    assert isinstance(snap, NameLookupSnapshot)
    with pytest.raises(MontyRuntimeError) as exc_info:
        snap.resume()
    assert exc_info.value.display(format='msg') == snapshot("name 'missing' is not defined")


def test_double_resume_raises(session: MontySession):
    snap = session.feed_start('f()')
    assert isinstance(snap, FunctionSnapshot)
    snap.resume({'return_value': 1})
    with pytest.raises(RuntimeError) as exc_info:
        snap.resume({'return_value': 2})
    assert str(exc_info.value) == snapshot('snapshot has already been resumed')


def test_dump_after_resume_raises(session: MontySession):
    # a resumed snapshot is a spent cursor — dumping it would serialize the
    # advanced session state, not this suspension, so it is rejected
    snap = session.feed_start('f()')
    assert isinstance(snap, FunctionSnapshot)
    snap.dump()  # dumping a live (un-resumed) snapshot is fine
    snap.resume({'return_value': 1})
    with pytest.raises(RuntimeError) as exc_info:
        snap.dump()
    assert str(exc_info.value) == snapshot('cannot dump a snapshot that has already been resumed')


def test_feed_while_suspended_raises(session: MontySession):
    session.feed_start('f()')
    with pytest.raises(RuntimeError) as exc_info:
        session.feed_run('1 + 1')
    assert exc_info.value.args[0] == snapshot(
        'monty worker protocol error: feed called while a suspension is awaiting an answer'
    )


def test_resume_has_no_mount_arg(session: MontySession):
    # mounts are fixed for the feed: resume takes no mount=, so passing one is
    # a plain unexpected-keyword TypeError (go through Any to bypass the stub).
    snap = session.feed_start('f()')
    assert isinstance(snap, FunctionSnapshot)
    untyped_snap: Any = snap
    with pytest.raises(TypeError) as exc_info:
        untyped_snap.resume({'return_value': 1}, mount=[])
    assert exc_info.value.args[0] == snapshot("FunctionSnapshot.resume() got an unexpected keyword argument 'mount'")
    # the rejected call did not consume the snapshot — it can still be answered
    done = snap.resume({'return_value': 1})
    assert isinstance(done, MontyComplete)


def test_os_call_surfaces_without_handler(session: MontySession):
    snap = session.feed_start("from pathlib import Path\nPath('/etc/x').read_text()")
    assert isinstance(snap, FunctionSnapshot)
    assert snap.is_os_function == snapshot(True)
    assert snap.function_name == snapshot('Path.read_text')


def test_os_handler_auto_dispatched(session: MontySession):
    def handle_os(name: OsFunction, args: tuple[Any, ...], kwargs: dict[str, Any]) -> str:
        assert name == 'Path.read_text'
        return 'file body'

    snap = session.feed_start(
        "from pathlib import Path\nPath('/data/x').read_text()",
        os=handle_os,
    )
    assert isinstance(snap, MontyComplete)
    assert snap.output == snapshot('file body')


def test_future_mechanism_sync(session: MontySession):
    code = 'import asyncio\nasync def main():\n    return await go()\nasyncio.run(main())'
    snap = session.feed_start(code)
    assert isinstance(snap, FunctionSnapshot)
    assert snap.function_name == snapshot('go')
    call_id = snap.call_id
    nxt = snap.resume({'future': ...})
    assert isinstance(nxt, FutureSnapshot)
    assert nxt.pending_call_ids == [call_id]
    done = nxt.resume({call_id: {'return_value': 99}})
    assert isinstance(done, MontyComplete)
    assert done.output == snapshot(99)


def test_future_cannot_resolve_to_future(session: MontySession):
    # a future must settle to a value/exception; resolving with another future
    # is rejected up front, without consuming the (single-use) snapshot
    code = 'import asyncio\nasync def main():\n    return await go()\nasyncio.run(main())'
    snap = session.feed_start(code)
    assert isinstance(snap, FunctionSnapshot)
    call_id = snap.call_id
    nxt = snap.resume({'future': ...})
    assert isinstance(nxt, FutureSnapshot)
    # a future result is not a settled result, so the stub rejects it too —
    # go through Any to exercise the runtime guard
    untyped_nxt: Any = nxt
    with pytest.raises(TypeError) as exc_info:
        untyped_nxt.resume({call_id: {'future': ...}})
    # message embeds the runtime call id, so compare directly rather than snapshot
    assert (
        exc_info.value.args[0]
        == f'future {call_id} cannot resolve to another future; provide a return value or exception'
    )
    # the rejected resolution did not consume the snapshot — it still resolves
    done = nxt.resume({call_id: {'return_value': 7}})
    assert isinstance(done, MontyComplete)
    assert done.output == snapshot(7)


def test_dump_at_suspension_then_load_and_resume(pool: Monty):
    with pool.checkout() as session:
        snap = session.feed_start('y = fetch()\ny + 1')
        assert isinstance(snap, FunctionSnapshot)
        blob = snap.dump()

    with pool.checkout() as session:
        loaded_snap = session.load_snapshot(blob)
        assert isinstance(loaded_snap, FunctionSnapshot)
        assert loaded_snap.function_name == snapshot('fetch')
        done = loaded_snap.resume({'return_value': 41})
        assert isinstance(done, MontyComplete)
        assert done.output == snapshot(42)


def test_loaded_snapshot_reports_the_dumps_script_name(pool: Monty):
    # script_name travels inside the dump; the restored snapshot reports the
    # dump's name, not the (differently-configured) restoring session's
    with pool.checkout(script_name='original.py') as session:
        snap = session.feed_start('fetch()')
        assert isinstance(snap, FunctionSnapshot)
        assert snap.script_name == snapshot('original.py')
        blob = snap.dump()

    with pool.checkout(script_name='different.py') as session:
        loaded_snap = session.load_snapshot(blob)
        assert isinstance(loaded_snap, FunctionSnapshot)
        assert loaded_snap.script_name == snapshot('original.py')


def test_mounts_restored_on_load_when_resupplied(pool: Monty, tmp_path: Path):
    # Re-supplying the feed's mounts to load_snapshot rebuilds the mount table,
    # so the mounted read after resume is served in-worker and never surfaces.
    (tmp_path / 'hello.txt').write_text('hi')
    mount = MountDir('/data', str(tmp_path), mode='read-only')
    code = "f()\nfrom pathlib import Path\nPath('/data/hello.txt').read_text()"
    with pool.checkout() as session:
        snap = session.feed_start(code, mount=mount)
        assert isinstance(snap, FunctionSnapshot)
        assert snap.function_name == snapshot('f')
        blob = snap.dump()

    with pool.checkout() as session:
        loaded_snap = session.load_snapshot(blob, mount=mount)
        assert isinstance(loaded_snap, FunctionSnapshot)
        done = loaded_snap.resume({'return_value': None})
        assert isinstance(done, MontyComplete)
        assert done.output == snapshot('hi')


def test_load_errors_when_required_mount_not_resupplied(pool: Monty, tmp_path: Path):
    # Omitting a mount the suspended feed had is a loud error, not a silent
    # drop — load validates the dump's recorded mount requirements.
    (tmp_path / 'hello.txt').write_text('hi')
    mount = MountDir('/data', str(tmp_path), mode='read-only')
    code = "f()\nfrom pathlib import Path\nPath('/data/hello.txt').read_text()"
    with pool.checkout() as session:
        snap = session.feed_start(code, mount=mount)
        assert isinstance(snap, FunctionSnapshot)
        blob = snap.dump()

    with pool.checkout() as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.load_snapshot(blob)
        # a failed load (here a validation error) poisons the session
        with pytest.raises(RuntimeError):
            session.feed_run('1 + 1')
    assert exc_info.value.display(format='msg') == snapshot(
        'the dump was suspended with a mount at "/data" that was not re-supplied to load; pass the same mounts the original feed used'
    )


def test_load_errors_when_mount_supplied_to_idle_dump(pool: Monty, tmp_path: Path):
    # An idle dump has no mount requirements, so supplying one is rejected.
    with pool.checkout() as session:
        session.feed_run('kept = 1')
        blob = session.dump()

    mount = MountDir('/data', str(tmp_path), mode='read-only')
    with pool.checkout() as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.load_snapshot(blob, mount=mount)
    assert exc_info.value.display(format='msg') == snapshot(
        'a mount at "/data" was supplied to load but the dump\'s feed had no such mount'
    )


def test_load_restores_idle_session(pool: Monty):
    with pool.checkout() as session:
        session.feed_run('kept = 7')
        blob = session.dump()

    with pool.checkout() as session:
        assert session.load(blob) is None
        assert session.feed_run('kept + 1') == snapshot(8)


def test_load_snapshot_on_idle_dump_raises(pool: Monty):
    with pool.checkout() as session:
        session.feed_run('kept = 1')
        blob = session.dump()

    with pool.checkout() as session:
        with pytest.raises(RuntimeError) as exc_info:
            session.load_snapshot(blob)
        assert str(exc_info.value) == snapshot('this dump is an idle session — use load() to restore it')
        # the failed load poisons the session — it is not retryable
        with pytest.raises(RuntimeError):
            session.feed_run('1 + 1')


def test_load_idle_dump_after_a_suspended_dump_path(pool: Monty):
    # the converse mismatch: load() on a suspended snapshot raises
    with pool.checkout() as session:
        snap = session.feed_start('f()')
        assert isinstance(snap, FunctionSnapshot)
        blob = snap.dump()

    with pool.checkout() as session:
        with pytest.raises(RuntimeError) as exc_info:
            session.load(blob)
        assert str(exc_info.value) == snapshot('this dump is a suspended snapshot — use load_snapshot() to resume it')
        # the failed load poisons the session — it is not retryable
        with pytest.raises(RuntimeError):
            session.feed_run('1 + 1')


def test_load_after_feed_raises(pool: Monty):
    # load / load_snapshot are only valid on a fresh, undriven session.
    with pool.checkout() as session:
        blob = session.dump()
    with pool.checkout() as session:
        session.feed_run('x = 1')
        with pytest.raises(RuntimeError) as exc_info:
            session.load_snapshot(blob)
        assert str(exc_info.value) == snapshot(
            'load / load_snapshot is only valid on a fresh session, before any feed_run / feed_start / load / load_snapshot'
        )


async def test_async_function_call_suspends_then_completes():
    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            snap = await session.feed_start('x = add(2, 3)\nx * 10')
            assert isinstance(snap, AsyncFunctionSnapshot)
            assert snap.function_name == snapshot('add')
            done = await snap.resume({'return_value': 5})
            assert isinstance(done, MontyComplete)
            assert done.output == snapshot(50)


async def test_async_future_mechanism():
    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            code = 'import asyncio\nasync def main():\n    return await go()\nasyncio.run(main())'
            snap = await session.feed_start(code)
            assert isinstance(snap, AsyncFunctionSnapshot)
            call_id = snap.call_id
            nxt = await snap.resume({'future': ...})
            assert isinstance(nxt, AsyncFutureSnapshot)
            assert nxt.pending_call_ids == [call_id]
            done = await nxt.resume({call_id: {'return_value': 99}})
            assert isinstance(done, MontyComplete)
            assert done.output == snapshot(99)


async def test_async_dump_load_round_trip():
    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            await session.feed_start('y = fetch()\ny + 1')
            blob = await session.dump()

        async with pool.checkout() as session:
            loaded_snap = await session.load_snapshot(blob)
            assert isinstance(loaded_snap, AsyncFunctionSnapshot)
            done = await loaded_snap.resume({'return_value': 41})
            assert isinstance(done, MontyComplete)
            assert done.output == snapshot(42)
