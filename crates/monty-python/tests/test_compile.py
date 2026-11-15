"""Compile-only validation through the subprocess client."""

import pytest
from inline_snapshot import snapshot

from pydantic_monty import AsyncMonty, FunctionSnapshot, Monty, MontyComplete, MontyRuntimeError, MontySyntaxError

CODE = """\
print('RAN')
def f(x=print('default ran')): pass
@unknown_decorator()
def g(): pass
unknown_callback()
while True: pass
"""


def test_compile_only(pool: Monty, capfd: pytest.CaptureFixture[str]):
    with pool.checkout() as session:
        session.feed_run('kept = 41')
        before = session.dump()
        assert session.compile(CODE) is None
        assert session.compile('') is None
        assert session.compile('import unavailable_module') is None
        assert session.compile('value + 1', input_names=['value']) is None
        assert session.dump() == before
        with pytest.raises(MontyRuntimeError, match='NotImplementedError') as exc_info:
            session.compile('\nclass B(A): pass')
        frame = exc_info.value.traceback()[0]
        assert frame.line == snapshot(2)
        assert frame.column == snapshot(1)
        assert isinstance(exc_info.value.exception(), NotImplementedError)
        assert exc_info.value.display(format='type-msg') == snapshot(
            'NotImplementedError: The monty syntax parser does not yet support class inheritance and metaclasses'
        )
        with pytest.raises(MontySyntaxError) as syntax_info:
            session.compile('def broken(:')
        assert syntax_info.value.display(format='msg') == snapshot(
            'Expected a parameter or the end of the parameter list'
        )
        assert session.dump() == before
        assert session.feed_run('kept + 1') == snapshot(42)
        with pytest.raises(MontyRuntimeError, match="name 'f' is not defined"):
            session.feed_run('f')
    assert capfd.readouterr().out == snapshot('')


def test_compile_keeps_fresh_session_restorable(pool: Monty):
    with pool.checkout() as original:
        original.feed_run('kept = 41')
        saved = original.dump()

    with pool.checkout() as fresh:
        assert fresh.compile('print("not executed")') is None
        with pytest.raises(MontySyntaxError):
            fresh.compile('def broken(:')
        fresh.load_session(saved)
        assert fresh.feed_run('kept + 1') == snapshot(42)


async def test_compile_only_async(capfd: pytest.CaptureFixture[str]):
    async with AsyncMonty() as pool, pool.checkout() as session:
        assert await session.compile(CODE) is None
        with pytest.raises(MontyRuntimeError, match='NotImplementedError'):
            await session.compile('class B(A): pass')
        with pytest.raises(MontySyntaxError):
            await session.compile('def broken(:')
        assert await session.feed_run('40 + 2') == snapshot(42)
    assert capfd.readouterr().out == snapshot('')


def test_compile_preserves_suspended_feed(pool: Monty):
    with pool.checkout() as session:
        pending = session.feed_start('callback() + 1')
        assert isinstance(pending, FunctionSnapshot)
        with pytest.raises(RuntimeError, match='compile called while a suspension is awaiting an answer'):
            session.compile('42')
        complete = pending.resume({'return_value': 41})
        assert isinstance(complete, MontyComplete)
        assert complete.output == snapshot(42)
