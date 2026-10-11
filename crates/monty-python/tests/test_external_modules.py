"""Tests for `external_modules`: host modules the sandbox imports, paired with
the module stubs that type-check them."""

from __future__ import annotations

import asyncio
import functools
import inspect
import types
from collections.abc import Awaitable, Callable
from typing import Any

import pytest
from inline_snapshot import snapshot

from pydantic_monty import (
    AsyncMonty,
    ClassInstance,
    ClassType,
    ExternalModule,
    Monty,
    MontyComplete,
    MontyRuntimeError,
    MontyTypingError,
)


def add(a: int, b: int) -> int:
    return a + b


def concat(a: str, b: str) -> str:
    return a + b


TOOLS: dict[str, Any] = {'add': add, 'concat': concat, 'VERSION': 3}
MODULES = {'tools': ExternalModule(TOOLS)}
CODE = "import tools\nfrom tools import concat\n[tools.add(1, 2), concat('a', b='b'), tools.VERSION]"
ADD_STUB = 'def add(a: int, b: int) -> int: ...\n'


def tools_module() -> types.ModuleType:
    module = types.ModuleType('tools')
    for name, value in TOOLS.items():
        setattr(module, name, value)
    return module


class _Tools:
    def reveal(self) -> str:
        return 'hidden'


def test_import_binds_the_host_module(pool: Monty):
    with pool.checkout() as session:
        assert session.feed_run(CODE, external_modules=MODULES) == snapshot([3, 'ab', 3])


@pytest.mark.parametrize(
    ('tools', 'message'),
    [
        pytest.param(
            ExternalModule(tools_module()),  # pyright: ignore[reportArgumentType]
            snapshot("external_modules['tools'].module must be a dict or a ClassInstance, not module"),
            id='module',
        ),
        pytest.param(
            ExternalModule(types.SimpleNamespace(**TOOLS)),  # pyright: ignore[reportArgumentType]
            snapshot("external_modules['tools'].module must be a dict or a ClassInstance, not SimpleNamespace"),
            id='namespace',
        ),
        pytest.param(
            ExternalModule(_Tools),  # pyright: ignore[reportArgumentType]
            snapshot("external_modules['tools'].module must be a dict or a ClassInstance, not the class _Tools"),
            id='class',
        ),
        pytest.param(
            _Tools,
            snapshot(
                "external_modules['tools'] must be an ExternalModule or a callable returning one, not the class _Tools"
            ),
            id='class-entry',
        ),
        pytest.param(
            TOOLS,
            snapshot("external_modules['tools'] must be an ExternalModule or a callable returning one, not dict"),
            id='bare-dict',
        ),
        pytest.param(
            ExternalModule(TOOLS, stubs=1),  # pyright: ignore[reportArgumentType]
            snapshot("external_modules['tools'].stubs must be a str or None, not int"),
            id='stubs-not-str',
        ),
    ],
)
def test_other_module_shapes_are_rejected(pool: Monty, tools: Any, message: str):
    # Dicts explicitly select exports; modules and arbitrary objects could expose imports.
    # Classes construct instances, so they are rejected as factories before execution.
    with pool.checkout() as session:
        with pytest.raises(TypeError) as exc_info:
            session.feed_run('1', external_modules={'tools': tools})
        assert str(exc_info.value) == message


def test_a_module_factory_runs_once_per_feed_when_first_needed(pool: Monty):
    calls = 0

    def factory() -> ExternalModule:
        nonlocal calls
        calls += 1
        return ExternalModule(TOOLS)

    modules = {'tools': factory}
    with pool.checkout() as session:
        assert session.feed_run('1', external_modules=modules) == snapshot(1)
        assert calls == 0
        code = 'import tools\nimport tools as t\n[tools.add(1, 2), t.VERSION]'
        assert session.feed_run(code, external_modules=modules) == snapshot([3, 3])
        assert calls == 1
        # the result stands for the module for the rest of the feed; the next feed asks again
        assert session.feed_run('import tools\ntools.VERSION', external_modules=modules) == snapshot(3)
        assert calls == 2
        # a host that wants one lookup per session memoizes the factory itself
        cached = {'tools': functools.cache(factory)}
        for _ in range(2):
            assert session.feed_run('import tools\ntools.VERSION', external_modules=cached) == snapshot(3)
        assert calls == 3


async def _async_tools() -> ExternalModule:
    await asyncio.sleep(0)
    return ExternalModule(TOOLS)


class _AwaitableTools:
    """An awaitable that is not a coroutine, as a factory may return."""

    closed = 0

    def __await__(self) -> Any:
        return _async_tools().__await__()

    def close(self) -> None:
        # not a coroutine's `close`: the binding must not call it when it refuses the awaitable
        type(self).closed += 1


async def _async_add(a: int, b: int) -> int:
    await asyncio.sleep(0)
    return a + b


def _sub(a: int, b: int) -> int:
    return a - b


async def _async_tools_with_async_add() -> ExternalModule:
    await asyncio.sleep(0)
    return ExternalModule({'add': _async_add, 'sub': _sub})


async def test_an_awaitable_factory_serves_a_call_through_a_restored_binding():
    # The restored binding retains its function names, but the host module is gone.
    # Calling it must await the new factory before awaiting the function.
    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            await session.feed_run(
                'import tools', external_modules={'tools': ExternalModule({'add': add, 'sub': _sub})}
            )
            blob = await session.dump()
        async with pool.checkout() as session:
            await session.load_session(blob)
            code = '[await tools.add(1, 2), tools.sub(5, 3), await tools.add(3, 4)]'
            modules = {'tools': _async_tools_with_async_add}
            assert await asyncio.wait_for(session.feed_run(code, external_modules=modules), 5) == snapshot([3, 2, 7])


async def _async_pkg() -> ExternalModule:
    await asyncio.sleep(0)
    return ExternalModule({'VERSION': 1, 'sub': ExternalModule({'add': _async_add})})


async def test_an_awaitable_factory_serves_a_call_into_a_submodule_through_a_restored_binding():
    # as above, through a binding into a submodule: the call names
    # `pkg.sub.add`, and it is `pkg` whose factory is awaited
    async with AsyncMonty() as pool:
        pkg = ExternalModule({'VERSION': 1, 'sub': ExternalModule({'add': add})})
        async with pool.checkout() as session:
            await session.feed_run('import pkg.sub', external_modules={'pkg': pkg})
            blob = await session.dump()
        async with pool.checkout() as session:
            await session.load_session(blob)
            code = '[await pkg.sub.add(1, 2), pkg.VERSION, await pkg.sub.add(3, 4)]'
            modules = {'pkg': _async_pkg}
            assert await asyncio.wait_for(session.feed_run(code, external_modules=modules), 5) == snapshot([3, 1, 7])


@pytest.mark.parametrize('awaitable', [_async_tools, _AwaitableTools], ids=['coroutine', 'awaitable'])
async def test_an_async_module_factory_is_awaited(awaitable: Callable[[], Awaitable[ExternalModule]]):
    calls = 0

    def factory() -> Awaitable[ExternalModule]:
        nonlocal calls
        calls += 1
        return awaitable()

    code = 'import tools\nimport tools as t\n[tools.add(1, 2), t.VERSION]'
    async with AsyncMonty() as pool:
        async with pool.checkout() as session:
            assert await session.feed_run(code, external_modules={'tools': factory}) == snapshot([3, 3])
            assert calls == 1


@pytest.mark.parametrize(
    ('factory', 'message'),
    [
        pytest.param(lambda: 1 / 0, snapshot('ZeroDivisionError: division by zero'), id='raises'),
        pytest.param(
            _async_tools,
            snapshot(
                "RuntimeError: external_modules['tools']() returned an awaitable; async module factories require AsyncMonty"
            ),
            id='coroutine-on-sync-pool',
        ),
        pytest.param(
            lambda: _AwaitableTools(),
            snapshot(
                "RuntimeError: external_modules['tools']() returned an awaitable; async module factories require AsyncMonty"
            ),
            id='awaitable-on-sync-pool',
        ),
        pytest.param(
            lambda: 3,
            snapshot("TypeError: external_modules['tools']() returned int, not an ExternalModule"),
            id='not-a-module',
        ),
        pytest.param(
            lambda: ExternalModule({'a.b': add}),
            snapshot("ValueError: external_modules entries must have identifier keys, not 'a.b'"),
            id='bad-key',
        ),
    ],
)
def test_a_module_factory_failure_raises_at_the_import(pool: Monty, factory: Any, message: str):
    """Factory results undergo the same validation as direct entries, at import time."""
    with pool.checkout() as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import tools', external_modules={'tools': factory})
        assert str(exc_info.value) == message
        assert _AwaitableTools.closed == 0
        assert session.feed_run('1 + 1') == snapshot(2)


def test_submodules_are_attributes_of_their_parent(pool: Monty):
    sub = ExternalModule({'add': add, 'VERSION': 2})
    pkg = ExternalModule({'VERSION': 1, 'sub': sub})
    with pool.checkout() as session:
        code = (
            'import pkg.sub\n'
            'import pkg.sub as s\n'
            'from pkg import sub\n'
            'from pkg.sub import add, VERSION\n'
            '[pkg.VERSION, pkg.sub.VERSION, s.add(1, 2), sub.add(3, 4), add(5, 6), VERSION, type(pkg.sub).__name__]'
        )
        assert session.feed_run(code, external_modules={'pkg': pkg}) == snapshot([1, 2, 3, 7, 11, 2, 'pkg.sub'])
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import pkg.nope', external_modules={'pkg': pkg})
        assert str(exc_info.value) == snapshot("ModuleNotFoundError: No module named 'pkg.nope'")
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('from pkg.sub.deeper import x', external_modules={'pkg': pkg})
        assert str(exc_info.value) == snapshot("ModuleNotFoundError: No module named 'pkg.sub.deeper'")
        # a submodule's stand-in answers a call on itself like any module's
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import pkg.sub\npkg.sub()', external_modules={'pkg': pkg})
        assert str(exc_info.value) == snapshot("TypeError: 'pkg.sub' object is not callable")


def test_a_class_instance_submodule(pool: Monty):
    class Tools:
        def add(self, a: int, b: int) -> int:
            return a + b

    pkg = ExternalModule({'sub': ExternalModule(ClassInstance(Tools(), allowed_methods={'add'}))})
    with pool.checkout() as session:
        assert session.feed_run('from pkg import sub\nsub.add(2, 3)', external_modules={'pkg': pkg}) == snapshot(5)


@pytest.mark.parametrize(
    ('entry', 'error', 'message'),
    [
        pytest.param(
            ExternalModule({'sub': ExternalModule(lambda: TOOLS)}),  # pyright: ignore[reportArgumentType]
            TypeError,
            snapshot("external_modules['pkg.sub'].module must be a dict or a ClassInstance, not function"),
            id='nested-factory',
        ),
        pytest.param(
            ExternalModule({'1sub': ExternalModule(TOOLS)}),
            ValueError,
            snapshot("external_modules entries must have identifier keys, not '1sub'"),
            id='nested-name',
        ),
        pytest.param(
            ExternalModule({'sub': ExternalModule(TOOLS, stubs=1)}),  # pyright: ignore[reportArgumentType]
            TypeError,
            snapshot("external_modules['pkg.sub'].stubs must be a str or None, not int"),
            id='nested-stubs',
        ),
        pytest.param(
            ExternalModule({'a.b': add}),
            ValueError,
            snapshot("external_modules entries must have identifier keys, not 'a.b'"),
            id='dotted-key',
        ),
        pytest.param(
            ExternalModule({'for': ExternalModule(TOOLS)}),
            ValueError,
            snapshot('external_modules[\'pkg.for\']: module name "for" is not a valid identifier'),
            id='keyword-submodule',
        ),
        pytest.param(
            ExternalModule({'json': ExternalModule(TOOLS)}),
            ValueError,
            snapshot(
                'external_modules[\'pkg.json\']: module "json" is provided by the sandbox or its type checker and cannot be replaced'
            ),
            id='sandbox-name-submodule',
        ),
    ],
)
def test_invalid_submodules_are_rejected(pool: Monty, entry: ExternalModule, error: type[Exception], message: str):
    with pool.checkout() as session:
        with pytest.raises(error) as exc_info:
            session.feed_run('1', external_modules={'pkg': entry})
        assert str(exc_info.value) == message


def test_external_modules_is_any_mapping(pool: Monty):
    with pool.checkout() as session:
        assert session.feed_run(CODE, external_modules=types.MappingProxyType(MODULES)) == snapshot([3, 'ab', 3])
        with pytest.raises(TypeError) as exc_info:
            session.feed_run('1', external_modules=[('tools', MODULES['tools'])])  # pyright: ignore[reportArgumentType]
        assert str(exc_info.value) == snapshot(
            'external_modules must be a mapping of module name to ExternalModule, not list'
        )


def test_cyclic_submodules_are_rejected(pool: Monty):
    """An entry nested inside itself is refused at the feed rather than walked forever."""
    module: dict[str, Any] = {}
    pkg = ExternalModule(module)
    module['sub'] = ExternalModule({'pkg': pkg})
    with pool.checkout() as session:
        with pytest.raises(ValueError) as exc_info:
            session.feed_run('1', external_modules={'pkg': pkg})
    assert str(exc_info.value) == snapshot(
        "external_modules['pkg.sub.pkg'] is external_modules['pkg'] again: modules cannot nest cyclically"
    )


@pytest.mark.parametrize(
    ('tools', 'name', 'message'),
    [
        pytest.param(
            {'_secret': lambda: 'hidden'},
            'tools._secret',
            snapshot("NameError: name 'tools._secret' is not defined"),
            id='private',
        ),
        pytest.param(
            ClassInstance(_Tools()),
            'tools.reveal',
            snapshot("NameError: name 'tools.reveal' is not defined"),
            id='class-instance',
        ),
    ],
)
def test_name_based_calls_respect_module_exposure(pool: Monty, tools: Any, name: str, message: str):
    """Forged dotted calls cannot reach private names or bypass a `ClassInstance` wrapper
    policy; instance methods must route by UUID."""

    def probe() -> str:
        return 'hidden'

    probe.__name__ = name
    with pool.checkout() as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run(
                'import tools\nprobe()', inputs={'probe': probe}, external_modules={'tools': ExternalModule(tools)}
            )
        assert str(exc_info.value) == message


def test_a_forged_name_with_many_dots_is_matched_against_the_configured_modules(pool: Monty):
    """Resolution stops at the first missing component, even for a long forged name."""

    def probe() -> str:
        return 'hidden'

    probe.__name__ = 'a.' * 5000 + 'f'
    modules = {'tools': ExternalModule(TOOLS), 'a': ExternalModule({'a': ExternalModule(TOOLS)})}
    with pool.checkout() as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('probe()', inputs={'probe': probe}, external_modules=modules)
        assert str(exc_info.value) == snapshot(f"NameError: name '{probe.__name__}' is not defined")
        assert session.feed_run('from a.a import add\nadd(1, 2)', external_modules=modules) == snapshot(3)


def test_the_module_object(pool: Monty):
    code = 'import tools\nimport tools as t\n[tools.add is t.add, type(tools).__name__, hasattr(tools, "nope")]'
    with pool.checkout() as session:
        assert session.feed_run(code, external_modules=MODULES) == snapshot([True, 'tools', False])


def test_import_errors(pool: Monty):
    with pool.checkout() as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import nope', external_modules=MODULES)
        assert str(exc_info.value) == snapshot("ModuleNotFoundError: No module named 'nope'")
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('from tools import nope', external_modules=MODULES)
        assert str(exc_info.value) == snapshot("ImportError: cannot import name 'nope' from 'tools' (unknown location)")
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run("import tools\ntools.add('x', 1)", external_modules=MODULES)
        assert str(exc_info.value) == snapshot('TypeError: can only concatenate str (not "int") to str')
        # no modules at all
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import tools')
        assert str(exc_info.value) == snapshot("ModuleNotFoundError: No module named 'tools'")


def test_a_dict_module_is_not_callable_and_has_no_other_methods(pool: Monty):
    """Dict-module stand-ins have no host object to answer method calls."""
    with pool.checkout() as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import tools\ntools()', external_modules=MODULES)
        assert str(exc_info.value) == snapshot("TypeError: 'tools' object is not callable")
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import tools\ntools.nope()', external_modules=MODULES)
        assert str(exc_info.value) == snapshot("AttributeError: 'tools' object has no attribute 'nope'")


def test_a_class_type_in_a_module_dict_is_a_type(pool: Monty):
    """A `ClassType` wrapper crosses as the type it wraps, while a bare class is a
    host function that constructs on the host."""

    class Config:
        def __init__(self, key: str) -> None:
            self.key = key

    tools = {'Config': ClassType(Config, init=True), 'Plain': Config}
    with pool.checkout() as session:
        code = 'import tools\nc = tools.Config("k")\n[isinstance(c, tools.Config), type(c).__name__, type(tools.Plain).__name__]'
        assert session.feed_run(code, external_modules={'tools': ExternalModule(tools)}) == snapshot(
            [True, 'Config', 'function']
        )


def test_a_class_instance_module(pool: Monty):
    class Tools:
        def add(self, a: int, b: int) -> int:
            return a + b

    tools = ClassInstance(Tools(), allowed_methods={'add'})
    with pool.checkout() as session:
        assert session.feed_run('import tools\ntools.add(2, 3)', external_modules={'tools': ExternalModule(tools)}) == (
            snapshot(5)
        )


def test_resume_auto_answers_imports(pool: Monty):
    """`resume_auto` answers import and stub-request snapshots from captured modules."""
    stubs = ADD_STUB + 'def concat(a: str, b: str) -> str: ...\nVERSION: int\n'
    modules = {'tools': ExternalModule(TOOLS, stubs=stubs)}
    with pool.checkout(type_check=True) as session:
        step = session.feed_start(CODE, external_modules=modules)
        calls: list[str] = []
        while not isinstance(step, MontyComplete):
            calls.append(repr(step))
            step = step.resume_auto()
        assert step.output == snapshot([3, 'ab', 3])
        assert calls == snapshot(
            [
                'FunctionSnapshot(function_name="import_stubs", is_os_function=true)',
                'FunctionSnapshot(function_name="import", is_os_function=true)',
                'FunctionSnapshot(function_name="import", is_os_function=true)',
                'FunctionSnapshot(function_name="tools.add", is_os_function=false)',
                'FunctionSnapshot(function_name="tools.concat", is_os_function=false)',
            ]
        )


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
        modules = {'tools': ExternalModule({'first': first, 'second': second})}
        async with pool.checkout() as session:
            result = await asyncio.wait_for(session.feed_run(code, external_modules=modules), 5)
    assert result == snapshot([1, 2])


def test_module_stubs_type_check_imports(pool: Monty):
    modules = {'tools': ExternalModule(TOOLS, stubs=ADD_STUB)}
    with pool.checkout(type_check=True, type_check_format='concise') as session:
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run("from tools import add\nadd('x', 2)", external_modules=modules)
        assert str(exc_info.value) == snapshot(
            'main.py:2:5: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`\n'
        )
        assert session.feed_run('import tools\ntools.add(1, 2)', external_modules=modules) == snapshot(3)
        # the import committed by that feed is still bound for the next check
        assert session.feed_run('tools.add(3, 4)', external_modules=modules) == snapshot(7)


def test_module_stubs_are_fetched_lazily(pool: Monty):
    """Stub requests run factories before type checking; imports reuse their results.
    Later feeds reuse the stubs, but call the factory again for imports."""
    calls = 0

    def factory() -> ExternalModule:
        nonlocal calls
        calls += 1
        return ExternalModule(TOOLS, stubs=ADD_STUB)

    modules = {'tools': factory}
    with pool.checkout(type_check=True, type_check_format='concise') as session:
        assert session.feed_run('1', external_modules=modules) == snapshot(1)
        assert calls == 0
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run("import tools\ntools.add('x', 2)", external_modules=modules)
        assert str(exc_info.value) == snapshot(
            'main.py:2:11: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`\n'
        )
        assert calls == 1
        assert session.feed_run('import tools\ntools.add(1, 2)', external_modules=modules) == snapshot(3)
        # Once for the failed feed's stub request, once for the next feed's import.
        assert calls == 2


def test_the_os_handler_never_sees_a_module_ask(pool: Monty):
    def os_handler(**kwargs: Any) -> Any:
        raise AssertionError(f'unexpected OS call {kwargs}')

    modules = {'tools': ExternalModule(TOOLS, stubs=ADD_STUB)}
    with pool.checkout(type_check=True) as session:
        assert session.feed_run('import tools\ntools.add(1, 2)', external_modules=modules, os=os_handler) == snapshot(3)


async def test_async_factories_are_awaited_together_for_a_stubs_ask():
    ready = asyncio.Event()

    async def tools() -> ExternalModule:
        await ready.wait()
        return ExternalModule(TOOLS, stubs=ADD_STUB)

    async def other() -> ExternalModule:
        ready.set()
        return ExternalModule({'VERSION': 2}, stubs='VERSION: int\n')

    modules = {'tools': tools, 'other': other}
    code = 'import tools, other\ntools.add(other.VERSION, 1)'
    async with AsyncMonty() as pool:
        async with pool.checkout(type_check=True) as session:
            assert await asyncio.wait_for(session.feed_run(code, external_modules=modules), 5) == snapshot(3)


async def test_a_factory_error_closes_the_coroutines_of_a_stubs_ask():
    """An error ends the ask, so the coroutines made for it are closed rather than left unawaited."""
    coroutines: list[Any] = []

    async def make_tools() -> ExternalModule:
        return ExternalModule(TOOLS, stubs=ADD_STUB)

    def tools() -> Awaitable[ExternalModule]:
        coroutines.append(make_tools())
        return coroutines[-1]

    def other() -> Awaitable[ExternalModule]:
        raise ValueError('no other')

    async with AsyncMonty() as pool:
        async with pool.checkout(type_check=True) as session:
            with pytest.raises(MontyRuntimeError) as exc_info:
                await session.feed_run('import tools, other', external_modules={'tools': tools, 'other': other})
    assert str(exc_info.value) == snapshot('ValueError: no other')
    assert [inspect.getcoroutinestate(coro) for coro in coroutines] == snapshot(['CORO_CLOSED'])


def test_submodule_stubs_type_check_as_a_package(pool: Monty):
    sub = ExternalModule({'add': add}, stubs=ADD_STUB)
    pkg = ExternalModule({'VERSION': 1, 'sub': sub}, stubs='VERSION: int\n')
    with pool.checkout(type_check=True, type_check_format='concise') as session:
        code = 'import pkg.sub\nfrom pkg import VERSION\npkg.sub.add(1, VERSION)'
        assert session.feed_run(code, external_modules={'pkg': pkg}) == snapshot(2)
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run("from pkg.sub import add\nadd('x', 2)", external_modules={'pkg': pkg})
        assert str(exc_info.value) == snapshot(
            'main.py:2:5: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`\n'
        )


def test_a_module_without_stubs_does_not_type_check(pool: Monty):
    with pool.checkout(type_check=True, type_check_format='concise') as session:
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run('import tools', external_modules=MODULES)
        assert str(exc_info.value) == snapshot(
            'main.py:1:8: error[unresolved-import] Cannot resolve imported module `tools`\n'
        )
        # asked about once per session: a stub supplied later is not consulted
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run('import tools', external_modules={'tools': ExternalModule(TOOLS, stubs=ADD_STUB)})
        assert str(exc_info.value) == snapshot(
            'main.py:1:8: error[unresolved-import] Cannot resolve imported module `tools`\n'
        )


def test_module_stubs_ride_in_a_dump(pool: Monty):
    modules = {'tools': ExternalModule(TOOLS, stubs=ADD_STUB)}
    with pool.checkout(type_check=True, type_check_format='concise') as session:
        assert session.feed_run('import tools\ntools.add(1, 2)', external_modules=modules) == snapshot(3)
        blob = session.dump()

    # the dump brings its own type checking, stubs and committed import; the
    # module itself is host state, so the restored session's feeds supply it again
    with pool.checkout() as session:
        assert session.load_session(blob) is None
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run("tools.add('x', 2)", external_modules=MODULES)
        assert str(exc_info.value) == snapshot(
            'main.py:1:11: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`\n'
        )
        assert session.feed_run('tools.add(3, 4)', external_modules=MODULES) == snapshot(7)


@pytest.mark.parametrize(
    ('module', 'message'),
    [
        ('json', snapshot('module "json" is provided by the sandbox or its type checker and cannot be replaced')),
        ('1tools', snapshot('module name "1tools" is not a valid identifier')),
    ],
)
def test_invalid_module_names(pool: Monty, module: str, message: str):
    with pool.checkout() as session:
        for entry in (ExternalModule({}, stubs=''), ExternalModule({})):
            with pytest.raises(ValueError) as exc_info:
                session.feed_run('1', external_modules={module: entry})
            assert str(exc_info.value) == message


def test_a_module_the_type_checker_alone_provides_is_served_without_a_stub(pool: Monty):
    # the runtime asks the host for `abc`, so a host may serve it
    abc = ExternalModule({'VERSION': 1})
    with pool.checkout() as session:
        assert session.feed_run('import abc\nabc.VERSION', external_modules={'abc': abc}) == snapshot(1)
    # the checker keeps typeshed's stub for it, which an entry's stub cannot replace
    with pool.checkout(type_check=True, type_check_format='concise') as session:
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run(
                'import abc\nabc.VERSION', external_modules={'abc': ExternalModule({'VERSION': 1}, stubs='')}
            )
        assert str(exc_info.value) == snapshot(
            'main.py:2:1: error[unresolved-attribute] Module `abc` has no member `VERSION`\n'
        )
