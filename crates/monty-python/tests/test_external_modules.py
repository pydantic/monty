"""Tests for `external_modules`: host modules the sandbox imports, paired with
the module stubs that type-check them."""

from __future__ import annotations

import asyncio
import inspect
import types
import uuid
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
    MontyConversionError,
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


class _Key(str):
    """A `str` whose hash and equality are its identity, so two spellings of one name coexist in a dict."""

    def __hash__(self) -> int:
        return id(self)

    def __eq__(self, other: object) -> bool:
        return self is other


class _Tools:
    def reveal(self) -> str:
        return 'hidden'


def test_import_binds_the_host_module(pool: Monty):
    with pool.checkout(external_modules=MODULES) as session:
        assert session.feed_run(CODE) == snapshot([3, 'ab', 3])


@pytest.mark.parametrize(
    ('attrs', 'kwargs', 'message'),
    [
        pytest.param(tools_module(), {}, snapshot('ExternalModule attrs must be a dict, not module'), id='module'),
        pytest.param(
            types.SimpleNamespace(**TOOLS),
            {},
            snapshot('ExternalModule attrs must be a dict, not SimpleNamespace'),
            id='namespace',
        ),
        pytest.param(_Tools, {}, snapshot('ExternalModule attrs must be a dict, not type'), id='class'),
        pytest.param(
            TOOLS,
            {'stubs': 1},
            snapshot("'int' object is not an instance of 'str'"),
            id='stubs-not-str',
        ),
        pytest.param(
            TOOLS,
            {'id': 'not-a-uuid'},
            snapshot('ExternalModule id must be a uuid.UUID'),
            id='id-not-uuid',
        ),
        pytest.param({1: add}, {}, snapshot('ExternalModule attrs must have str keys, not int'), id='int-key'),
        pytest.param(
            {'a.b': add}, {}, snapshot("ExternalModule attrs must have identifier keys, not 'a.b'"), id='dotted-key'
        ),
        pytest.param(lambda: TOOLS, {}, snapshot('ExternalModule attrs must be a dict, not function'), id='factory'),
        pytest.param(
            {'1sub': ExternalModule(TOOLS)},
            {},
            snapshot("ExternalModule attrs must have identifier keys, not '1sub'"),
            id='nested-name',
        ),
        pytest.param(
            {'for': add},
            {},
            snapshot("ExternalModule attrs must have identifier keys, not 'for'"),
            id='keyword-key',
        ),
        pytest.param(
            {_Key('add'): add, _Key('add'): concat},
            {},
            snapshot("ExternalModule attrs has the key 'add' twice"),
            id='duplicate-key',
        ),
        pytest.param(
            {'json': ExternalModule(TOOLS)},
            {},
            snapshot(
                'ExternalModule attrs[\'json\']: module "json" is provided by the sandbox or its type checker and cannot be replaced'
            ),
            id='sandbox-name-submodule',
        ),
    ],
)
def test_invalid_modules_are_rejected_at_construction(attrs: Any, kwargs: dict[str, Any], message: str):
    """Attrs are checked once, when the module is made: a dict with identifier keys,
    submodules under valid module names, and arbitrary objects refused since they could
    expose imports."""
    with pytest.raises((TypeError, ValueError)) as exc_info:
        ExternalModule(attrs, **kwargs)
    assert str(exc_info.value) == message


@pytest.mark.parametrize(
    ('tools', 'message'),
    [
        pytest.param(
            _Tools,
            snapshot(
                "TypeError: external_modules['tools'] must be an ExternalModule or a callable returning one, not the class _Tools"
            ),
            id='class-entry',
        ),
        pytest.param(
            TOOLS,
            snapshot(
                "TypeError: external_modules['tools'] must be an ExternalModule or a callable returning one, not dict"
            ),
            id='bare-dict',
        ),
        pytest.param(
            None,
            snapshot(
                "TypeError: external_modules['tools'] must be an ExternalModule or a callable returning one, not NoneType"
            ),
            id='none',
        ),
    ],
)
def test_other_entry_shapes_are_rejected(pool: Monty, tools: Any, message: str):
    # Classes construct instances, so they are rejected as factories, at the import that reaches them.
    with pool.checkout(external_modules={'tools': tools}) as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import tools')
        assert str(exc_info.value) == message


def test_the_module_object_is_frozen():
    uid = uuid.uuid4()
    tools = ExternalModule({'add': add}, stubs=ADD_STUB, id=uid)
    assert tools.id == uid
    assert tools.stubs == ADD_STUB
    assert tools.attrs == {'add': add}
    assert repr(tools) == snapshot(
        f"ExternalModule({{'add': {add!r}}}, stubs='def add(a: int, b: int) -> int: ...\\n', id=UUID('{uid}'))"
    )
    assert tools == ExternalModule({'add': add}, stubs=ADD_STUB, id=uid)
    assert tools != ExternalModule({'add': add}, stubs=ADD_STUB)
    assert tools != ExternalModule({'add': add}, id=uid)
    # `attrs` is a copy, so neither the dict given nor the one read back changes the module
    tools.attrs['VERSION'] = 1
    assert tools.attrs == {'add': add}
    assert ExternalModule({}).id != ExternalModule({}).id


def test_a_module_factory_runs_once_per_session_when_first_needed(pool: Monty):
    calls = 0

    def factory() -> ExternalModule:
        nonlocal calls
        calls += 1
        return ExternalModule(TOOLS)

    with pool.checkout(external_modules={'tools': factory}) as session:
        assert session.feed_run('1') == snapshot(1)
        assert calls == 0
        code = 'import tools\nimport tools as t\n[tools.add(1, 2), t.VERSION]'
        assert session.feed_run(code) == snapshot([3, 3])
        assert calls == 1
        # the result stands for the module for the rest of the session
        assert session.feed_run('import tools\ntools.VERSION') == snapshot(3)
        assert calls == 1


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
        async with pool.checkout(external_modules={'tools': ExternalModule({'add': add, 'sub': _sub})}) as session:
            await session.feed_run('import tools')
            blob = await session.dump()
        async with pool.checkout(external_modules={'tools': _async_tools_with_async_add}) as session:
            await session.load_session(blob)
            code = '[await tools.add(1, 2), tools.sub(5, 3), await tools.add(3, 4)]'
            assert await asyncio.wait_for(session.feed_run(code), 5) == snapshot([3, 2, 7])


async def _async_pkg() -> ExternalModule:
    await asyncio.sleep(0)
    return ExternalModule({'VERSION': 1, 'sub': ExternalModule({'add': _async_add})})


async def test_an_awaitable_factory_serves_a_call_into_a_submodule_through_a_restored_binding():
    # as above, through a binding into a submodule: the call names
    # `pkg.sub.add`, and it is `pkg` whose factory is awaited
    async with AsyncMonty() as pool:
        pkg = ExternalModule({'VERSION': 1, 'sub': ExternalModule({'add': add})})
        async with pool.checkout(external_modules={'pkg': pkg}) as session:
            await session.feed_run('import pkg.sub')
            blob = await session.dump()
        async with pool.checkout(external_modules={'pkg': _async_pkg}) as session:
            await session.load_session(blob)
            code = '[await pkg.sub.add(1, 2), pkg.VERSION, await pkg.sub.add(3, 4)]'
            assert await asyncio.wait_for(session.feed_run(code), 5) == snapshot([3, 1, 7])


@pytest.mark.parametrize('awaitable', [_async_tools, _AwaitableTools], ids=['coroutine', 'awaitable'])
async def test_an_async_module_factory_is_awaited(awaitable: Callable[[], Awaitable[ExternalModule]]):
    calls = 0

    def factory() -> Awaitable[ExternalModule]:
        nonlocal calls
        calls += 1
        return awaitable()

    code = 'import tools\nimport tools as t\n[tools.add(1, 2), t.VERSION]'
    async with AsyncMonty() as pool:
        async with pool.checkout(external_modules={'tools': factory}) as session:
            assert await session.feed_run(code) == snapshot([3, 3])
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
            snapshot("ValueError: ExternalModule attrs must have identifier keys, not 'a.b'"),
            id='bad-key',
        ),
    ],
)
def test_a_module_factory_failure_raises_at_the_import(pool: Monty, factory: Any, message: str):
    """Factory results undergo the same validation as direct entries, at import time."""
    with pool.checkout(external_modules={'tools': factory}) as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import tools')
        assert str(exc_info.value) == message
        assert _AwaitableTools.closed == 0
        assert session.feed_run('1 + 1') == snapshot(2)


def test_submodules_are_attributes_of_their_parent(pool: Monty):
    sub = ExternalModule({'add': add, 'VERSION': 2})
    pkg = ExternalModule({'VERSION': 1, 'sub': sub})
    with pool.checkout(external_modules={'pkg': pkg}) as session:
        code = (
            'import pkg.sub\n'
            'import pkg.sub as s\n'
            'from pkg import sub\n'
            'from pkg.sub import add, VERSION\n'
            '[pkg.VERSION, pkg.sub.VERSION, s.add(1, 2), sub.add(3, 4), add(5, 6), VERSION, type(pkg.sub).__name__]'
        )
        assert session.feed_run(code) == snapshot([1, 2, 3, 7, 11, 2, 'module'])
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import pkg.nope')
        assert str(exc_info.value) == snapshot("ModuleNotFoundError: No module named 'pkg.nope'")
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('from pkg.sub.deeper import x')
        assert str(exc_info.value) == snapshot("ModuleNotFoundError: No module named 'pkg.sub.deeper'")
        # a submodule is a module object like its parent
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import pkg.sub\npkg.sub()')
        assert str(exc_info.value) == snapshot("TypeError: 'module' object is not callable")
        # an attribute that is not a module is not a submodule
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import pkg.VERSION')
        assert str(exc_info.value) == snapshot("ModuleNotFoundError: No module named 'pkg.VERSION'")


def test_a_class_instance_in_a_submodule(pool: Monty):
    class Tools:
        def add(self, a: int, b: int) -> int:
            return a + b

    tools = ClassInstance(Tools(), allowed_methods={'add'})
    pkg = ExternalModule({'sub': ExternalModule({'tools': tools})})
    with pool.checkout(external_modules={'pkg': pkg}) as session:
        assert session.feed_run('from pkg.sub import tools\ntools.add(2, 3)') == snapshot(5)


def test_external_modules_is_any_mapping(pool: Monty):
    with pool.checkout(external_modules=types.MappingProxyType(MODULES)) as session:
        assert session.feed_run(CODE) == snapshot([3, 'ab', 3])
    with pytest.raises(TypeError) as exc_info:
        pool.checkout(external_modules=[('tools', MODULES['tools'])])  # pyright: ignore[reportArgumentType]
    assert str(exc_info.value) == snapshot(
        'external_modules must be a mapping of module name to ExternalModule, not list'
    )


def test_attrs_are_copied_at_construction(pool: Monty):
    """A module is frozen once made, so later edits to the dict given (a cycle included)
    never reach the sandbox."""
    module: dict[str, Any] = {'VERSION': 1}
    pkg = ExternalModule(module)
    module['VERSION'] = 2
    module['sub'] = ExternalModule({'pkg': pkg})
    with pool.checkout(external_modules={'pkg': pkg}) as session:
        assert session.feed_run('import pkg\npkg.VERSION') == snapshot(1)
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import pkg.sub')
        assert str(exc_info.value) == snapshot("ModuleNotFoundError: No module named 'pkg.sub'")


def test_entries_added_after_the_checkout_are_importable(pool: Monty):
    """The mapping is read live, so a host discovering a module mid-session can offer it."""
    modules: dict[str, Any] = {'tools': ExternalModule(TOOLS)}
    with pool.checkout(external_modules=modules) as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import other')
        assert str(exc_info.value) == snapshot("ModuleNotFoundError: No module named 'other'")
        modules['other'] = ExternalModule({'VERSION': 1})
        assert session.feed_run('import other\nother.VERSION') == snapshot(1)


def test_name_based_calls_respect_module_exposure(pool: Monty):
    def probe() -> str:
        return 'hidden'

    probe.__name__ = 'tools._secret'
    with pool.checkout(external_modules={'tools': ExternalModule({'_secret': lambda: 'hidden'})}) as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import tools\nprobe()', inputs={'probe': probe})
        assert str(exc_info.value) == snapshot("NameError: name 'tools._secret' is not defined")


def test_a_forged_name_with_many_dots_is_matched_against_the_configured_modules(pool: Monty):
    """Resolution stops at the first missing component, even for a long forged name."""

    def probe() -> str:
        return 'hidden'

    probe.__name__ = 'a.' * 5000 + 'f'
    modules = {'tools': ExternalModule(TOOLS), 'a': ExternalModule({'a': ExternalModule(TOOLS)})}
    with pool.checkout(external_modules=modules) as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('probe()', inputs={'probe': probe})
        assert str(exc_info.value) == snapshot(f"NameError: name '{probe.__name__}' is not defined")
        assert session.feed_run('from a.a import add\nadd(1, 2)') == snapshot(3)


def test_the_module_object(pool: Monty):
    code = 'import tools\nimport tools as t\n[tools.add is t.add, type(tools).__name__, hasattr(tools, "nope"), repr(tools)]'
    with pool.checkout(external_modules=MODULES) as session:
        assert session.feed_run(code) == snapshot([True, 'module', False, "<module 'tools'>"])


def test_the_module_crosses_back_with_its_id(pool: Monty):
    with pool.checkout(external_modules=MODULES) as session:
        tools = session.feed_run('import tools\ntools')
        assert isinstance(tools, ExternalModule)
        assert tools.id == MODULES['tools'].id
        assert tools.stubs is None
        assert list(tools.attrs) == snapshot(['add', 'concat', 'VERSION'])
        assert tools.attrs['VERSION'] == snapshot(3)
        # attrs are the sandbox's values: a host function comes back as its name
        assert tools.attrs['add'] == snapshot('tools.add')
        assert session.feed_run('import tools\nimport tools as t\ntools is t') == snapshot(False)


def test_a_module_is_not_a_value(pool: Monty):
    with pool.checkout() as session:
        with pytest.raises(MontyConversionError) as exc_info:
            session.feed_run('x', inputs={'x': ExternalModule({})})
        assert str(exc_info.value) == snapshot(
            'an ExternalModule enters the sandbox only through external_modules, not as a value'
        )


def test_import_errors(pool: Monty):
    with pool.checkout(external_modules=MODULES) as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import nope')
        assert str(exc_info.value) == snapshot("ModuleNotFoundError: No module named 'nope'")
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('from tools import nope')
        assert str(exc_info.value) == snapshot("ImportError: cannot import name 'nope' from 'tools' (unknown location)")
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run("import tools\ntools.add('x', 1)")
        assert str(exc_info.value) == snapshot('TypeError: can only concatenate str (not "int") to str')
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import tools.VERSION')
        assert str(exc_info.value) == snapshot("ModuleNotFoundError: No module named 'tools.VERSION'")
    # no modules at all
    with pool.checkout() as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import tools')
        assert str(exc_info.value) == snapshot("ModuleNotFoundError: No module named 'tools'")


def test_a_module_is_not_callable_and_names_missing_attributes(pool: Monty):
    """Module errors are raised in the sandbox, as CPython's are, with no host round trip."""
    with pool.checkout(external_modules=MODULES) as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import tools\ntools()')
        assert str(exc_info.value) == snapshot("TypeError: 'module' object is not callable")
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('import tools\ntools.nope()')
        assert str(exc_info.value) == snapshot("AttributeError: module 'tools' has no attribute 'nope'")


def test_a_class_type_in_a_module_dict_is_a_type(pool: Monty):
    """A `ClassType` wrapper crosses as the type it wraps, while a bare class is a
    host function that constructs on the host."""

    class Config:
        def __init__(self, key: str) -> None:
            self.key = key

    tools = {'Config': ClassType(Config, init=True), 'Plain': Config}
    with pool.checkout(external_modules={'tools': ExternalModule(tools)}) as session:
        code = 'import tools\nc = tools.Config("k")\n[isinstance(c, tools.Config), type(c).__name__, type(tools.Plain).__name__]'
        assert session.feed_run(code) == snapshot([True, 'Config', 'function'])


def test_a_replaced_entry_is_seen_on_the_next_import(pool: Monty):
    modules = {'tools': ExternalModule({'VERSION': 1})}
    with pool.checkout(external_modules=modules) as session:
        assert session.feed_run('import tools\ntools.VERSION') == snapshot(1)
        modules['tools'] = ExternalModule({'VERSION': 2})
        assert session.feed_run('import tools\ntools.VERSION') == snapshot(2)


def test_resume_auto_answers_imports(pool: Monty):
    """`resume_auto` answers import and stub-request snapshots from the session's modules."""
    stubs = ADD_STUB + 'def concat(a: str, b: str) -> str: ...\nVERSION: int\n'
    modules = {'tools': ExternalModule(TOOLS, stubs=stubs)}
    with pool.checkout(type_check=True, external_modules=modules) as session:
        step = session.feed_start(CODE)
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
        async with pool.checkout(external_modules=modules) as session:
            result = await asyncio.wait_for(session.feed_run(code), 5)
    assert result == snapshot([1, 2])


def test_module_stubs_type_check_imports(pool: Monty):
    modules = {'tools': ExternalModule(TOOLS, stubs=ADD_STUB)}
    with pool.checkout(type_check=True, type_check_format='concise', external_modules=modules) as session:
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run("from tools import add\nadd('x', 2)")
        assert str(exc_info.value) == snapshot(
            'main.py:2:5: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`\n'
        )
        assert session.feed_run('import tools\ntools.add(1, 2)') == snapshot(3)
        # the import committed by that feed is still bound for the next check
        assert session.feed_run('tools.add(3, 4)') == snapshot(7)


def test_module_stubs_are_fetched_lazily(pool: Monty):
    """The stubs ask runs the factory before type checking; the session reuses its result."""
    calls = 0

    def factory() -> ExternalModule:
        nonlocal calls
        calls += 1
        return ExternalModule(TOOLS, stubs=ADD_STUB)

    modules = {'tools': factory}
    with pool.checkout(type_check=True, type_check_format='concise', external_modules=modules) as session:
        assert session.feed_run('1') == snapshot(1)
        assert calls == 0
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run("import tools\ntools.add('x', 2)")
        assert str(exc_info.value) == snapshot(
            'main.py:2:11: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`\n'
        )
        assert calls == 1
        assert session.feed_run('import tools\ntools.add(1, 2)') == snapshot(3)
        assert calls == 1


def test_the_os_handler_never_sees_a_module_ask(pool: Monty):
    def os_handler(**kwargs: Any) -> Any:
        raise AssertionError(f'unexpected OS call {kwargs}')

    modules = {'tools': ExternalModule(TOOLS, stubs=ADD_STUB)}
    with pool.checkout(type_check=True, external_modules=modules) as session:
        assert session.feed_run('import tools\ntools.add(1, 2)', os=os_handler) == snapshot(3)


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
        async with pool.checkout(type_check=True, external_modules=modules) as session:
            assert await asyncio.wait_for(session.feed_run(code), 5) == snapshot(3)


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
        async with pool.checkout(type_check=True, external_modules={'tools': tools, 'other': other}) as session:
            with pytest.raises(MontyRuntimeError) as exc_info:
                await session.feed_run('import tools, other')
    assert str(exc_info.value) == snapshot('ValueError: no other')
    assert [inspect.getcoroutinestate(coro) for coro in coroutines] == snapshot(['CORO_CLOSED'])


def test_submodule_stubs_type_check_as_a_package(pool: Monty):
    sub = ExternalModule({'add': add}, stubs=ADD_STUB)
    pkg = ExternalModule({'VERSION': 1, 'sub': sub}, stubs='VERSION: int\n')
    with pool.checkout(type_check=True, type_check_format='concise', external_modules={'pkg': pkg}) as session:
        code = 'import pkg.sub\nfrom pkg import VERSION\npkg.sub.add(1, VERSION)'
        assert session.feed_run(code) == snapshot(2)
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run("from pkg.sub import add\nadd('x', 2)")
        assert str(exc_info.value) == snapshot(
            'main.py:2:5: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`\n'
        )


def test_a_module_without_stubs_does_not_type_check(pool: Monty):
    with pool.checkout(type_check=True, type_check_format='concise', external_modules=MODULES) as session:
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run('import tools')
        assert str(exc_info.value) == snapshot(
            'main.py:1:8: error[unresolved-import] Cannot resolve imported module `tools`\n'
        )
        blob = session.dump()
    # the answer is final for the session and travels in its dump: a stub supplied later is not asked for
    with pool.checkout(external_modules={'tools': ExternalModule(TOOLS, stubs=ADD_STUB)}) as session:
        session.load_session(blob)
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run('import tools')
        assert str(exc_info.value) == snapshot(
            'main.py:1:8: error[unresolved-import] Cannot resolve imported module `tools`\n'
        )


def test_module_stubs_ride_in_a_dump(pool: Monty):
    modules = {'tools': ExternalModule(TOOLS, stubs=ADD_STUB)}
    with pool.checkout(type_check=True, type_check_format='concise', external_modules=modules) as session:
        assert session.feed_run('import tools\ntools.add(1, 2)') == snapshot(3)
        blob = session.dump()

    # the dump brings its own type checking, stubs and committed import; the
    # module itself is host state, so the restoring checkout supplies it again
    with pool.checkout(external_modules=MODULES) as session:
        assert session.load_session(blob) is None
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run("tools.add('x', 2)")
        assert str(exc_info.value) == snapshot(
            'main.py:1:11: error[invalid-argument-type] Argument to function `add` is incorrect: Expected `int`, found `Literal["x"]`\n'
        )
        assert session.feed_run('tools.add(3, 4)') == snapshot(7)
        # the restored binding is still a module, carrying the id it was imported with
        assert session.feed_run('[type(tools).__name__, repr(tools)]') == snapshot(['module', "<module 'tools'>"])
        restored = session.feed_run('tools')
        assert isinstance(restored, ExternalModule)
        assert restored.id == modules['tools'].id
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('tools.nope', skip_type_check=True)
        assert str(exc_info.value) == snapshot("AttributeError: module 'tools' has no attribute 'nope'")


def test_a_sandbox_module_name_is_never_looked_up(pool: Monty):
    # the sandbox serves `json` itself, so the entry is unreachable, stub or not:
    # the checker knows `json` and never requests the host's stub for it
    modules = {'json': ExternalModule({'VERSION': 99}, stubs='VERSION: int\n')}
    with pool.checkout(type_check=True, type_check_format='concise', external_modules=modules) as session:
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run('import json\njson.VERSION')
        assert str(exc_info.value) == snapshot(
            'main.py:2:1: error[unresolved-attribute] Module `json` has no member `VERSION`\n'
        )
        assert session.feed_run("import json\nhasattr(json, 'VERSION')", skip_type_check=True) is False


@pytest.mark.parametrize(
    ('module', 'message'),
    [
        ('1tools', snapshot('ValueError: module name "1tools" is not a valid identifier')),
        ('for', snapshot('ValueError: module name "for" is not a valid identifier')),
        (
            'json',
            snapshot('ValueError: module "json" is provided by the sandbox or its type checker and cannot be replaced'),
        ),
    ],
)
def test_an_invalid_module_name_is_refused_when_a_call_reaches_it(pool: Monty, module: str, message: str):
    """No import can name such an entry; a forged dotted call is the only way to reach it."""

    def probe() -> str:
        return 'hidden'

    probe.__name__ = f'{module}.f'
    with pool.checkout(external_modules={module: ExternalModule({})}) as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('probe()', inputs={'probe': probe})
        assert str(exc_info.value) == message


def test_a_module_the_type_checker_alone_provides_is_served_without_a_stub(pool: Monty):
    # the runtime asks the host for `abc`, so a host may serve it
    with pool.checkout(external_modules={'abc': ExternalModule({'VERSION': 1})}) as session:
        assert session.feed_run('import abc\nabc.VERSION') == snapshot(1)
    # the checker keeps typeshed's stub for it, which an entry's stub cannot replace
    stubbed = {'abc': ExternalModule({'VERSION': 1}, stubs='')}
    with pool.checkout(type_check=True, type_check_format='concise', external_modules=stubbed) as session:
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run('import abc\nabc.VERSION')
        assert str(exc_info.value) == snapshot(
            'main.py:2:1: error[unresolved-attribute] Module `abc` has no member `VERSION`\n'
        )
