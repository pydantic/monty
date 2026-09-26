"""Tests for `external_modules`: host modules the sandbox imports, and the
module stubs that type-check them."""

from __future__ import annotations

import asyncio
import types
from typing import Any

import pytest
from inline_snapshot import snapshot

from pydantic_monty import AsyncMonty, ClassInstance, Monty, MontyComplete, MontyRuntimeError, MontyTypingError


def add(a: int, b: int) -> int:
    return a + b


def concat(a: str, b: str) -> str:
    return a + b


TOOLS: dict[str, Any] = {'add': add, 'concat': concat, 'VERSION': 3}
CODE = "import tools\nfrom tools import concat\n[tools.add(1, 2), concat('a', b='b'), tools.VERSION]"


def tools_module() -> types.ModuleType:
    module = types.ModuleType('tools')
    for name, value in TOOLS.items():
        setattr(module, name, value)
    return module


@pytest.mark.parametrize('tools', [TOOLS, types.SimpleNamespace(**TOOLS), tools_module()])
def test_import_binds_the_host_module(pool: Monty, tools: Any):
    with pool.checkout() as session:
        assert session.feed_run(CODE, external_modules={'tools': tools}) == snapshot([3, 'ab', 3])


def test_the_module_object(pool: Monty):
    code = 'import tools\nimport tools as t\n[tools.add is t.add, type(tools).__name__, hasattr(tools, "nope")]'
    with pool.checkout() as session:
        assert session.feed_run(code, external_modules={'tools': TOOLS}) == snapshot([True, 'tools', False])


def test_import_errors(pool: Monty):
    with pool.checkout() as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import nope', external_modules={'tools': TOOLS})
        assert str(exc_info.value) == snapshot("ModuleNotFoundError: No module named 'nope'")
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import tools')
        assert str(exc_info.value) == snapshot("ModuleNotFoundError: No module named 'tools'")
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('from tools import nope', external_modules={'tools': TOOLS})
        assert str(exc_info.value) == snapshot("ImportError: cannot import name 'nope' from 'tools' (unknown location)")
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run("import tools\ntools.add('x', 1)", external_modules={'tools': TOOLS})
        assert str(exc_info.value) == snapshot('TypeError: can only concatenate str (not "int") to str')


def test_a_class_instance_module(pool: Monty):
    class Tools:
        def add(self, a: int, b: int) -> int:
            return a + b

    tools = ClassInstance(Tools(), allowed_methods={'add'})
    with pool.checkout() as session:
        assert session.feed_run('import tools\ntools.add(2, 3)', external_modules={'tools': tools}) == snapshot(5)


def test_resume_auto_answers_imports(pool: Monty):
    with pool.checkout() as session:
        step = session.feed_start(CODE, external_modules={'tools': TOOLS})
        while not isinstance(step, MontyComplete):
            step = step.resume_auto()
        assert step.output == snapshot([3, 'ab', 3])


async def test_async_tools_run_concurrently():
    ready = asyncio.Event()

    async def first() -> int:
        await ready.wait()
        return 1

    async def second() -> int:
        ready.set()
        return 2

    code = 'import asyncio\nimport tools\nfrom tools import second\nawait asyncio.gather(tools.first(), second())'
    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            result = await asyncio.wait_for(
                session.feed_run(code, external_modules={'tools': {'first': first, 'second': second}}), 5
            )
    assert result == snapshot([1, 2])


def test_module_stubs_type_check_and_get_types(pool: Monty):
    stubs = {'tools': 'def add(a: int, b: int) -> int: ...\n'}
    with pool.checkout(type_check=True, type_check_format='concise', type_check_module_stubs=stubs) as session:
        assert session.get_types() == snapshot({'tools': 'def add(a: int, b: int) -> int: ...\n'})
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run("from tools import add\nadd('x', 2)", external_modules={'tools': TOOLS})
        assert str(exc_info.value) == snapshot(
            'main.py:2:5: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`\n'
        )
        assert session.feed_run('import tools\ntools.add(1, 2)', external_modules={'tools': TOOLS}) == snapshot(3)
        # the import committed by that feed is still bound for the next check
        assert session.feed_run('tools.add(3, 4)', external_modules={'tools': TOOLS}) == snapshot(7)


@pytest.mark.parametrize(
    ('module', 'message'),
    [
        ('json', snapshot('module "json" is provided by the sandbox and cannot take a stub')),
        ('1tools', snapshot('module stub name "1tools" is not a valid identifier')),
    ],
)
def test_invalid_module_stub_names(pool: Monty, module: str, message: str):
    with pytest.raises(ValueError) as exc_info:
        pool.checkout(type_check_module_stubs={module: ''})
    assert str(exc_info.value) == message
