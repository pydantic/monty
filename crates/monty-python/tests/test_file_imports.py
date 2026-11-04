"""Tests for importing modules from `.py` files: an import `external_modules`
does not answer searches its candidate files through the `os=` handler first
and the feed's mounts second."""

from __future__ import annotations

from pathlib import Path, PurePosixPath
from typing import Any

import pytest
from conftest import RunMonty
from inline_snapshot import snapshot

from pydantic_monty import NOT_HANDLED, AsyncMonty, ExternalModule, Monty, MontyComplete, MontyRuntimeError, MountDir


@pytest.fixture
def project(tmp_path: Path) -> Path:
    (tmp_path / 'helpers.py').write_text('def double(x):\n    return x * 2\n')
    (tmp_path / 'pkg').mkdir()
    (tmp_path / 'pkg' / '__init__.py').write_text('')
    (tmp_path / 'pkg' / 'sub.py').write_text('value = 3\n')
    return tmp_path


def test_a_mounted_module_imports(monty_run: RunMonty, project: Path):
    code = 'import helpers\nfrom pkg.sub import value\n(helpers.double(value), helpers.__file__, value)'
    with MountDir(host_path=str(project), virtual_path='/proj', mode='read-only') as mount:
        assert monty_run(code, mount=mount) == snapshot((6, '/proj/helpers.py', 3))


def test_the_os_handler_serves_module_files(monty_run: RunMonty):
    reads: list[Any] = []

    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
        reads.append((name, args))
        if args == (PurePosixPath('/served.py'),):
            return 'answer = 42\n'
        raise FileNotFoundError(f'No such file: {args[0]}')

    assert monty_run('import served\nserved.answer', os=os_handler) == snapshot(42)
    assert reads == snapshot(
        [
            ('Path.read_text', (PurePosixPath('/served/__init__.py'),)),
            ('Path.read_text', (PurePosixPath('/served.py'),)),
        ]
    )


def test_the_os_handler_sees_normalized_paths(monty_run: RunMonty):
    reads: list[Any] = []

    def os_handler(*, args: tuple[Any, ...], **_: Any) -> Any:
        reads.append(args[0])
        return "__path__ = ['/allowed/../private']\n" if args == (PurePosixPath('/pkg/__init__.py'),) else NOT_HANDLED

    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('import pkg.secrets', os=os_handler)
    assert str(exc_info.value) == snapshot("ModuleNotFoundError: No module named 'pkg.secrets'")
    # a `..` in a sandbox-set `__path__` is resolved before the handler sees it
    assert reads == snapshot(
        [
            PurePosixPath('/pkg/__init__.py'),
            PurePosixPath('/private/secrets/__init__.py'),
            PurePosixPath('/private/secrets.py'),
        ]
    )


def test_the_os_handler_is_read_before_mounts(monty_run: RunMonty, project: Path):
    def os_handler(*, name: str, args: tuple[Any, ...], **_: Any) -> Any:
        if args == (PurePosixPath('/proj/helpers.py'),):
            return "where = 'os'\n"
        return NOT_HANDLED

    with MountDir(host_path=str(project), virtual_path='/proj', mode='read-only') as mount:
        # the handler shadows the mount's `helpers.py`, and declines `pkg`, which the mount serves
        code = 'import helpers\nimport pkg.sub\n(helpers.where, pkg.sub.value)'
        assert monty_run(code, mount=mount, os=os_handler) == snapshot(('os', 3))


def test_host_modules_come_first(pool: Monty, project: Path):
    modules = {'helpers': ExternalModule({'where': 'host'})}
    with MountDir(host_path=str(project), virtual_path='/proj', mode='read-only') as mount:
        with pool.checkout(external_modules=modules) as session:
            assert session.feed_run('import helpers\nhelpers.where', mount=mount) == snapshot('host')


def test_a_read_error_raises_at_the_import(monty_run: RunMonty):
    def os_handler(**_: Any) -> Any:
        raise OSError('disk on fire')

    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('import helpers', os=os_handler)
    assert str(exc_info.value) == snapshot('OSError: disk on fire')


def test_a_missing_module_is_not_found(monty_run: RunMonty, project: Path):
    with MountDir(host_path=str(project), virtual_path='/proj', mode='read-only') as mount:
        with pytest.raises(MontyRuntimeError) as exc_info:
            monty_run('import nope', mount=mount)
        assert str(exc_info.value) == snapshot("ModuleNotFoundError: No module named 'nope'")
        with pytest.raises(MontyRuntimeError) as exc_info:
            monty_run('from pkg import nope', mount=mount)
        assert str(exc_info.value) == snapshot(
            "ImportError: cannot import name 'nope' from 'pkg' (/proj/pkg/__init__.py)"
        )


def test_resume_auto_searches_files(pool: Monty, project: Path):
    with MountDir(host_path=str(project), virtual_path='/proj', mode='read-only') as mount:
        with pool.checkout() as session:
            step = session.feed_start('import helpers\nhelpers.double(4)', mount=mount)
            while not isinstance(step, MontyComplete):
                step = step.resume_auto()
            assert step.output == snapshot(8)


async def test_an_async_os_handler_serves_module_files(project: Path):
    async def os_handler(*, args: tuple[Any, ...], **_: Any) -> Any:
        if args == (PurePosixPath('/proj/served.py'),):
            return 'answer = 42\n'
        return NOT_HANDLED

    code = 'import served\nimport helpers\n(served.answer, helpers.double(2))'
    async with AsyncMonty() as pool:
        with MountDir(host_path=str(project), virtual_path='/proj', mode='read-only') as mount:
            async with pool.checkout() as session:
                # `served` from the handler, `helpers` from the mount it declines
                assert await session.feed_run(code, mount=mount, os=os_handler) == snapshot((42, 4))
