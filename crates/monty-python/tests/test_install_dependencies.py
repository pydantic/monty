"""`session.install_dependencies` tests.

These run against the default `monty` *sandbox* worker, which has no host
interpreter to install for and rejects the request. A real install end-to-end is
covered by the `monty-cpython` Rust tests (an embedded-CPython worker). Here we
assert the rejection, the empty-list no-op, and that the session survives.
"""

from __future__ import annotations

from collections.abc import AsyncIterator

import pytest
from inline_snapshot import snapshot

from pydantic_monty import AsyncMonty, AsyncMontySession, MontyRuntimeError, MontySession


@pytest.fixture
async def asession() -> AsyncIterator[AsyncMontySession]:
    """A fresh checked-out async session for one test."""
    async with AsyncMonty() as pool, pool.checkout() as session:
        yield session


def test_install_dependencies_rejected_on_sandbox_worker(session: MontySession):
    with pytest.raises(MontyRuntimeError) as exc_info:
        session.install_dependencies(['httpx>=0.27'])
    assert exc_info.value.display(format='msg') == snapshot(
        'dependency installation is only supported by the CPython worker'
    )
    # the session survives the rejection and keeps working
    assert session.feed_run('1 + 1') == snapshot(2)


def test_install_dependencies_empty_is_a_noop(session: MontySession):
    # installing nothing trivially succeeds, even on the sandbox worker
    assert session.install_dependencies([]) is None
    assert session.feed_run('1 + 1') == snapshot(2)


@pytest.mark.parametrize(
    'requirement,message',
    [
        (
            '--index-url=http://evil',
            'invalid requirement "--index-url=http://evil": must not start with \'-\' (it would be parsed as a uv option)',
        ),
        (
            '-r /etc/hosts',
            'invalid requirement "-r /etc/hosts": must not start with \'-\' (it would be parsed as a uv option)',
        ),
        ('   ', 'invalid requirement "   ": must not be empty'),
    ],
)
def test_install_dependencies_validates_requirements(session: MontySession, requirement: str, message: str):
    # the pool rejects flag-like / empty requirements before sending anything to
    # the worker, so this fails the same way on the sandbox and CPython workers
    with pytest.raises(MontyRuntimeError) as exc_info:
        session.install_dependencies([requirement])
    assert exc_info.value.display(format='msg') == message
    # the session survives the rejection and keeps working
    assert session.feed_run('1 + 1') == snapshot(2)


async def test_async_install_dependencies_rejected_on_sandbox_worker(asession: AsyncMontySession):
    with pytest.raises(MontyRuntimeError) as exc_info:
        await asession.install_dependencies(['numpy'])
    assert exc_info.value.display(format='msg') == snapshot(
        'dependency installation is only supported by the CPython worker'
    )
    assert await asession.feed_run('1 + 1') == snapshot(2)


async def test_async_install_dependencies_empty_is_a_noop(asession: AsyncMontySession):
    assert await asession.install_dependencies([]) is None
    assert await asession.feed_run('1 + 1') == snapshot(2)
