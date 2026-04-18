import tempfile
from collections.abc import Generator
from pathlib import Path
from typing import Any, Callable, Literal

import pytest
from inline_snapshot import snapshot

import pydantic_monty
from pydantic_monty import NOT_HANDLED, MountDir

PrintCallback = Callable[[Literal['stdout'], str], None]


def make_print_collector() -> tuple[list[str], PrintCallback]:
    """Create a print callback that collects output into a list."""
    output: list[str] = []

    def callback(stream: Literal['stdout'], text: str) -> None:
        assert stream == 'stdout'
        output.append(text)

    return output, callback


# === Construction ===


def test_default_construction():
    repl = pydantic_monty.MontyRepl()
    assert repl.script_name == snapshot('main.py')


def test_custom_script_name():
    repl = pydantic_monty.MontyRepl(script_name='test.py')
    assert repl.script_name == snapshot('test.py')


def test_repr():
    repl = pydantic_monty.MontyRepl(script_name='my_repl.py')
    assert repr(repl) == snapshot("MontyRepl(script_name='my_repl.py')")


# === Basic feed_run behavior ===


def test_feed_run_expression_returns_value():
    repl = pydantic_monty.MontyRepl()
    assert repl.feed_run('1 + 2') == snapshot(3)


def test_feed_run_assignment_returns_none():
    repl = pydantic_monty.MontyRepl()
    assert repl.feed_run('x = 42') == snapshot(None)


def test_feed_run_empty_string_returns_none():
    repl = pydantic_monty.MontyRepl()
    assert repl.feed_run('') == snapshot(None)


def test_feed_run_none_literal():
    repl = pydantic_monty.MontyRepl()
    assert repl.feed_run('None') is None


# === State persistence across feeds ===


def test_variable_persists_across_feeds():
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 10')
    assert repl.feed_run('x') == snapshot(10)


def test_incremental_mutation():
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('counter = 0')
    repl.feed_run('counter = counter + 1')
    repl.feed_run('counter = counter + 1')
    assert repl.feed_run('counter') == snapshot(2)


def test_multiple_variables():
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 10')
    repl.feed_run('y = 20')
    assert repl.feed_run('x + y') == snapshot(30)


def test_function_defined_then_called():
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('def double(n):\n    return n * 2')
    assert repl.feed_run('double(21)') == snapshot(42)


def test_function_uses_previously_defined_variable():
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('factor = 3')
    repl.feed_run('def multiply(n):\n    return n * factor')
    assert repl.feed_run('multiply(7)') == snapshot(21)


def test_list_mutation_persists():
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('items = [1, 2, 3]')
    repl.feed_run('items.append(4)')
    assert repl.feed_run('len(items)') == snapshot(4)
    assert repl.feed_run('items') == snapshot([1, 2, 3, 4])


def test_dict_mutation_persists():
    repl = pydantic_monty.MontyRepl()
    repl.feed_run("data = {'a': 1}")
    repl.feed_run("data['b'] = 2")
    assert repl.feed_run('len(data)') == snapshot(2)
    assert repl.feed_run("data['b']") == snapshot(2)


def test_variable_reassignment():
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = "hello"')
    assert repl.feed_run('x') == snapshot('hello')
    repl.feed_run('x = 42')
    assert repl.feed_run('x') == snapshot(42)


# === Multi-statement snippets ===


def test_multi_statement_snippet():
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('a = 1\nb = 2\nc = a + b')
    assert repl.feed_run('c') == snapshot(3)


def test_loop_in_snippet():
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('total = 0\nfor i in range(5):\n    total = total + i')
    assert repl.feed_run('total') == snapshot(10)


def test_if_else_in_snippet():
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 10')
    repl.feed_run('result = "big" if x > 5 else "small"')
    assert repl.feed_run('result') == snapshot('big')


# === Return value types ===


@pytest.mark.parametrize(
    'code,expected',
    [
        ('42', 42),
        ('3.14', 3.14),
        ('"hello"', 'hello'),
        ('True', True),
        ('False', False),
        ('[1, 2, 3]', [1, 2, 3]),
        ('(1, 2, 3)', (1, 2, 3)),
        ("{'a': 1}", {'a': 1}),
    ],
    ids=['int', 'float', 'str', 'true', 'false', 'list', 'tuple', 'dict'],
)
def test_feed_run_return_types(code: str, expected: object):
    repl = pydantic_monty.MontyRepl()
    assert repl.feed_run(code) == expected


# === Error handling ===


def test_syntax_error():
    repl = pydantic_monty.MontyRepl()
    with pytest.raises(pydantic_monty.MontySyntaxError):
        repl.feed_run('def')


def test_runtime_error_preserves_state():
    """A runtime error should not destroy previously defined state."""
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 42')
    with pytest.raises(pydantic_monty.MontyRuntimeError):
        repl.feed_run('1 / 0')
    # x should still be accessible after the error
    assert repl.feed_run('x') == snapshot(42)


def test_name_error():
    repl = pydantic_monty.MontyRepl()
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        repl.feed_run('undefined_var')
    inner = exc_info.value.exception()
    assert isinstance(inner, NameError)


def test_type_error():
    repl = pydantic_monty.MontyRepl()
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        repl.feed_run('"hello" + 1')
    inner = exc_info.value.exception()
    assert isinstance(inner, TypeError)


def test_zero_division_error():
    repl = pydantic_monty.MontyRepl()
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        repl.feed_run('1 / 0')
    inner = exc_info.value.exception()
    assert isinstance(inner, ZeroDivisionError)


def test_index_error():
    repl = pydantic_monty.MontyRepl()
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        repl.feed_run('[1, 2][10]')
    inner = exc_info.value.exception()
    assert isinstance(inner, IndexError)


def test_key_error():
    repl = pydantic_monty.MontyRepl()
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        repl.feed_run("{'a': 1}['b']")
    inner = exc_info.value.exception()
    assert isinstance(inner, KeyError)


def test_multiple_errors_dont_corrupt_state():
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 1')
    with pytest.raises(pydantic_monty.MontyRuntimeError):
        repl.feed_run('1 / 0')
    repl.feed_run('x = x + 1')
    with pytest.raises(pydantic_monty.MontyRuntimeError):
        repl.feed_run('undefined_name')
    assert repl.feed_run('x') == snapshot(2)


def test_cross_snippet_traceback_resolves_against_defining_snippet():
    # REPL tracebacks must resolve each frame against the source text of the
    # snippet that actually produced the CodeRange byte offsets — not the
    # snippet that happens to be executing when the exception fires. The
    # function below is defined in snippet 0 (`<python-input-0>`), called
    # from snippet 1 (`<python-input-1>`); the raise-site frame needs to
    # point back at snippet 0's source for line/column/source_line.
    repl = pydantic_monty.MontyRepl()
    repl.feed_run("def f():\n    raise ValueError('boom')")
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        repl.feed_run('f()')
    frames = exc_info.value.traceback()
    assert [f.dict() for f in frames] == snapshot(
        [
            {
                'filename': '<python-input-1>',
                'line': 1,
                'column': 1,
                'end_line': 1,
                'end_column': 4,
                'function_name': '<module>',
                'source_line': 'f()',
            },
            {
                'filename': '<python-input-0>',
                'line': 2,
                'column': 11,
                'end_line': 2,
                'end_column': 29,
                'function_name': 'f',
                'source_line': "    raise ValueError('boom')",
            },
        ]
    )


# === Print callback ===


def test_print_callback_on_feed():
    repl = pydantic_monty.MontyRepl()
    output, callback = make_print_collector()
    repl.feed_run('print("hello")', print_callback=callback)
    assert ''.join(output) == snapshot('hello\n')


def test_print_callback_across_feeds():
    repl = pydantic_monty.MontyRepl()
    output, callback = make_print_collector()
    repl.feed_run('print("first")', print_callback=callback)
    repl.feed_run('print("second")', print_callback=callback)
    assert ''.join(output) == snapshot('first\nsecond\n')


# === Resource limits ===


def test_construction_with_limits():
    limits = pydantic_monty.ResourceLimits(max_duration_secs=5.0)
    repl = pydantic_monty.MontyRepl(limits=limits)
    assert repl.feed_run('1 + 1') == snapshot(2)


def test_infinite_loop_with_limits():
    limits = pydantic_monty.ResourceLimits(max_duration_secs=0.5)
    repl = pydantic_monty.MontyRepl(limits=limits)
    with pytest.raises(pydantic_monty.MontyRuntimeError):
        repl.feed_run('while True:\n    pass')


# === Serialization ===


def test_dump_load_roundtrip():
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 40')
    repl.feed_run('x = x + 1')

    serialized = repl.dump()
    assert isinstance(serialized, bytes)

    loaded = pydantic_monty.MontyRepl.load(serialized)
    assert loaded.feed_run('x + 1') == snapshot(42)


def test_dump_load_preserves_functions():
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('def greet(name):\n    return "hello " + name')

    loaded = pydantic_monty.MontyRepl.load(repl.dump())
    assert loaded.feed_run('greet("world")') == snapshot('hello world')


def test_dump_load_preserves_script_name():
    repl = pydantic_monty.MontyRepl(script_name='custom.py')
    loaded = pydantic_monty.MontyRepl.load(repl.dump())
    assert loaded.script_name == snapshot('custom.py')


def test_load_with_print_callback():
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 1')

    output, callback = make_print_collector()
    loaded = pydantic_monty.MontyRepl.load(repl.dump())
    loaded.feed_run('print(x)', print_callback=callback)
    assert ''.join(output) == snapshot('1\n')


def test_load_invalid_data():
    with pytest.raises(ValueError):
        pydantic_monty.MontyRepl.load(b'invalid data')


# === External functions ===


def test_external_function_basic():
    def add(a: int, b: int) -> int:
        return a + b

    repl = pydantic_monty.MontyRepl()
    assert repl.feed_run('result = add(3, 4)', external_functions={'add': add}) == snapshot(None)
    assert repl.feed_run('result') == snapshot(7)


def test_external_function_return_value():
    def greet(name: str) -> str:
        return f'hello {name}'

    repl = pydantic_monty.MontyRepl()
    assert repl.feed_run('greet("world")', external_functions={'greet': greet}) == snapshot('hello world')


def test_external_function_called_multiple_times():
    call_count = 0

    def counter():
        nonlocal call_count
        call_count += 1
        return call_count

    repl = pydantic_monty.MontyRepl()
    ext = {'counter': counter}
    assert repl.feed_run('counter()', external_functions=ext) == snapshot(1)
    assert repl.feed_run('counter()', external_functions=ext) == snapshot(2)
    assert call_count == 2


def test_external_function_persists_state_across_feeds():
    def double(x: int) -> int:
        return x * 2

    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 5')
    assert repl.feed_run('double(x)', external_functions={'double': double}) == snapshot(10)


def test_external_function_exception_becomes_runtime_error():
    def fail():
        raise ValueError('external failure')

    repl = pydantic_monty.MontyRepl()
    ext = {'fail': fail}
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        repl.feed_run('fail()', external_functions=ext)
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert str(inner) == snapshot('external failure')


def test_external_function_error_preserves_repl_state():
    def fail():
        raise ValueError('boom')

    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 42')
    ext = {'fail': fail}
    with pytest.raises(pydantic_monty.MontyRuntimeError):
        repl.feed_run('fail()', external_functions=ext)
    # REPL state should be preserved after error
    assert repl.feed_run('x') == snapshot(42)


def test_external_function_undefined_raises_name_error():
    """Calling a name that's not in external_functions raises NameError."""
    repl = pydantic_monty.MontyRepl()
    ext = {'known': lambda: 1}
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        repl.feed_run('unknown()', external_functions=ext)
    inner = exc_info.value.exception()
    assert isinstance(inner, NameError)


def test_external_function_with_print_callback():
    output, callback = make_print_collector()
    repl = pydantic_monty.MontyRepl()
    ext = {'get_msg': lambda: 'from external'}
    repl.feed_run('x = get_msg()\nprint(x)', external_functions=ext, print_callback=callback)
    assert ''.join(output) == snapshot('from external\n')


def test_external_function_with_kwargs():
    def greet(name: str, greeting: str = 'hello') -> str:
        return f'{greeting} {name}'

    repl = pydantic_monty.MontyRepl()
    ext = {'greet': greet}
    assert repl.feed_run("greet('world', greeting='hi')", external_functions=ext) == snapshot('hi world')


def test_feed_run_no_externals_with_os_preserves_repl_state():
    """feed_run with os= but no external_functions= preserves REPL state when an external call is hit.

    When os= is provided, feed_run uses the feed_start_loop path. If a non-OS external
    function is called but external_functions was not provided, the loop must restore
    the REPL before returning the error.
    """
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 42')

    # Provide os= to force the feed_start_loop path, but no external_functions
    def dummy_os(func: str, args: object, kwargs: object) -> None:
        pass

    with pytest.raises(RuntimeError, match='no external_functions provided'):
        repl.feed_run('unknown_func()', os=dummy_os)
    # REPL state must be preserved — previously this was lost
    assert repl.feed_run('x') == snapshot(42)


# === Inputs ===


def test_inputs_basic():
    repl = pydantic_monty.MontyRepl()
    assert repl.feed_run('x + 1', inputs={'x': 10}) == snapshot(11)


def test_inputs_used_in_same_snippet():
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('y = x + 1', inputs={'x': 42})
    assert repl.feed_run('y') == snapshot(43)


def test_inputs_multiple_values():
    repl = pydantic_monty.MontyRepl()
    assert repl.feed_run('a + b', inputs={'a': 3, 'b': 7}) == snapshot(10)


def test_inputs_override_existing_variable():
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 1')
    assert repl.feed_run('x', inputs={'x': 99}) == snapshot(99)


def test_inputs_with_external_functions():
    def double(n: int) -> int:
        return n * 2

    repl = pydantic_monty.MontyRepl()
    assert repl.feed_run('double(x)', inputs={'x': 5}, external_functions={'double': double}) == snapshot(10)


# === Tests for MontyRepl.feed_start() ===


def test_feed_start_no_external_calls():
    """feed_start with no external calls returns MontyComplete directly."""
    repl = pydantic_monty.MontyRepl()
    progress = repl.feed_start('1 + 2')
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output == snapshot(3)
    # REPL should still be usable
    assert repl.feed_run('3 + 4') == snapshot(7)


def test_feed_start_state_persists():
    """feed_start preserves REPL state from prior feed_run calls."""
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 10')
    progress = repl.feed_start('x + 5')
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output == snapshot(15)


def test_feed_start_external_function():
    """feed_start yields FunctionSnapshot for external function calls."""
    repl = pydantic_monty.MontyRepl()
    progress = repl.feed_start('add(1, 2)')
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('add')
    assert progress.args == snapshot((1, 2))
    progress = progress.resume({'return_value': 3})
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output == snapshot(3)
    # REPL should still be usable after
    assert repl.feed_run('1 + 1') == snapshot(2)


def test_feed_start_external_function_preserves_state():
    """feed_start async result is accessible in subsequent feed_run calls."""
    repl = pydantic_monty.MontyRepl()
    progress = repl.feed_start('result = add(1, 2)')
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    progress = progress.resume({'return_value': 42})
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert repl.feed_run('result') == snapshot(42)


def test_feed_start_multiple_external_calls():
    """feed_start handles multiple sequential external calls."""
    repl = pydantic_monty.MontyRepl()
    code = 'a = foo(1)\nb = bar(2)\na + b'
    progress = repl.feed_start(code)
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('foo')
    progress = progress.resume({'return_value': 10})
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('bar')
    progress = progress.resume({'return_value': 20})
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output == snapshot(30)


def test_feed_start_error_preserves_repl_state():
    """REPL state is preserved when feed_start raises an error."""
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 42')
    with pytest.raises(pydantic_monty.MontyRuntimeError):
        repl.feed_start('1 / 0')
    # REPL should still be usable
    assert repl.feed_run('x') == snapshot(42)


def test_feed_start_resume_error_preserves_repl_state():
    """REPL state is preserved when resume raises a runtime error."""
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 99')
    progress = repl.feed_start('fail()')
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    # Resume with an exception that isn't caught
    with pytest.raises(pydantic_monty.MontyRuntimeError):
        progress.resume({'exception': ValueError('boom')})
    assert repl.feed_run('x') == snapshot(99)


def test_feed_start_with_inputs():
    """feed_start supports the inputs parameter."""
    repl = pydantic_monty.MontyRepl()
    progress = repl.feed_start('process(x)', inputs={'x': 5})
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('process')
    assert progress.args == snapshot((5,))
    progress = progress.resume({'return_value': 25})
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output == snapshot(25)


def test_feed_start_with_print_callback():
    """feed_start supports the print_callback parameter."""
    output: list[tuple[str, str]] = []

    def callback(stream: str, text: str) -> None:
        output.append((stream, text))

    repl = pydantic_monty.MontyRepl()
    progress = repl.feed_start('print("hello")', print_callback=callback)
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert output == snapshot([('stdout', 'hello'), ('stdout', '\n')])


def test_feed_start_name_lookup():
    """feed_start yields NameLookupSnapshot for bare name access."""
    repl = pydantic_monty.MontyRepl()
    progress = repl.feed_start('x = foo')
    assert isinstance(progress, pydantic_monty.NameLookupSnapshot)
    assert progress.variable_name == snapshot('foo')
    progress = progress.resume(value=42)
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert repl.feed_run('x') == snapshot(42)


def test_feed_start_dump_load_repl_snapshot():
    """FunctionSnapshot from feed_start can be serialized and deserialized with load_repl_snapshot."""
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 10')
    progress = repl.feed_start('add(x, 2)')
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    data = progress.dump()
    loaded, loaded_repl = pydantic_monty.load_repl_snapshot(data)
    assert isinstance(loaded, pydantic_monty.FunctionSnapshot)
    assert loaded.function_name == snapshot('add')
    assert loaded.args == snapshot((10, 2))

    # Resume the loaded snapshot
    result = loaded.resume({'return_value': 12})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(12)

    # REPL state is restored and usable
    assert loaded_repl.feed_run('x') == snapshot(10)


def test_feed_start_dump_load_repl_snapshot_preserves_state():
    """REPL state from before feed_start is preserved through dump/load."""
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('counter = 0')
    repl.feed_run('counter = counter + 1')
    repl.feed_run('counter = counter + 1')

    progress = repl.feed_start('result = fetch(counter)')
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.args == snapshot((2,))

    data = progress.dump()
    loaded, loaded_repl = pydantic_monty.load_repl_snapshot(data)
    assert isinstance(loaded, pydantic_monty.FunctionSnapshot)
    result = loaded.resume({'return_value': 'done'})
    assert isinstance(result, pydantic_monty.MontyComplete)

    # Counter should still be 2, and result should be set
    assert loaded_repl.feed_run('counter') == snapshot(2)
    assert loaded_repl.feed_run('result') == snapshot('done')


def test_feed_start_dump_load_repl_snapshot_name_lookup():
    """NameLookupSnapshot from feed_start can be serialized and deserialized."""
    repl = pydantic_monty.MontyRepl()
    progress = repl.feed_start('x = foo')
    assert isinstance(progress, pydantic_monty.NameLookupSnapshot)
    assert progress.variable_name == snapshot('foo')

    data = progress.dump()
    loaded, loaded_repl = pydantic_monty.load_repl_snapshot(data)
    assert isinstance(loaded, pydantic_monty.NameLookupSnapshot)
    assert loaded.variable_name == snapshot('foo')

    result = loaded.resume(value=99)
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert loaded_repl.feed_run('x') == snapshot(99)


def test_feed_start_dump_load_repl_snapshot_multiple_calls():
    """Multiple external calls with dump/load between each."""
    repl = pydantic_monty.MontyRepl()
    progress = repl.feed_start('a = foo(1)\nb = bar(2)\na + b')

    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.function_name == snapshot('foo')

    # Dump/load between first and second call
    data = progress.dump()
    loaded, _ = pydantic_monty.load_repl_snapshot(data)
    assert isinstance(loaded, pydantic_monty.FunctionSnapshot)
    progress2 = loaded.resume({'return_value': 10})

    assert isinstance(progress2, pydantic_monty.FunctionSnapshot)
    assert progress2.function_name == snapshot('bar')

    # Dump/load between second call and completion
    data2 = progress2.dump()
    loaded2, _ = pydantic_monty.load_repl_snapshot(data2)
    assert isinstance(loaded2, pydantic_monty.FunctionSnapshot)
    result = loaded2.resume({'return_value': 20})

    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(30)


def test_feed_start_dump_load_snapshot_errors_on_repl():
    """load_snapshot rejects REPL snapshots — the wire formats are incompatible."""
    repl = pydantic_monty.MontyRepl()
    progress = repl.feed_start('fetch(1)')
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    data = progress.dump()
    # REPL snapshots use a different wire format, so load_snapshot fails on deserialization
    with pytest.raises(ValueError):
        pydantic_monty.load_snapshot(data)


def test_feed_start_dump_load_repl_snapshot_with_print_callback():
    """print_callback works on loaded REPL snapshots."""
    output, callback = make_print_collector()

    repl = pydantic_monty.MontyRepl()
    progress = repl.feed_start('x = fetch()')
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    data = progress.dump()
    loaded, loaded_repl = pydantic_monty.load_repl_snapshot(data, print_callback=callback)
    assert isinstance(loaded, pydantic_monty.FunctionSnapshot)
    # Resume — the loaded snapshot should use the print callback for subsequent prints
    loaded.resume({'return_value': 42})
    loaded_repl.feed_run('print(x)', print_callback=callback)
    assert ''.join(output) == snapshot('42\n')


def test_feed_start_dump_load_repl_snapshot_preserves_script_name():
    """Script name is preserved through REPL snapshot dump/load."""
    repl = pydantic_monty.MontyRepl(script_name='my_repl.py')
    progress = repl.feed_start('fetch()')
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    data = progress.dump()
    loaded, loaded_repl = pydantic_monty.load_repl_snapshot(data)
    assert loaded.script_name == snapshot('my_repl.py')
    assert loaded_repl.script_name == snapshot('my_repl.py')


def test_non_repl_dump_load_with_load_snapshot():
    """Non-REPL snapshots from Monty.start() work with load_snapshot."""
    m = pydantic_monty.Monty('func(1, 2)')
    progress = m.start()
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    data = progress.dump()
    loaded = pydantic_monty.load_snapshot(data)
    assert isinstance(loaded, pydantic_monty.FunctionSnapshot)
    assert loaded.function_name == snapshot('func')
    assert loaded.args == snapshot((1, 2))

    result = loaded.resume({'return_value': 100})
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(100)


def test_feed_start_dump_after_resume_fails():
    """Cannot dump a REPL snapshot that has already been resumed."""
    repl = pydantic_monty.MontyRepl()
    progress = repl.feed_start('fetch()')
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    progress.resume({'return_value': 1})

    with pytest.raises(RuntimeError) as exc_info:
        progress.dump()
    assert exc_info.value.args[0] == snapshot('Cannot dump progress that has already been resumed')


def test_inputs_various_types():
    repl = pydantic_monty.MontyRepl()
    assert repl.feed_run('s', inputs={'s': 'hello'}) == snapshot('hello')
    assert repl.feed_run('n', inputs={'n': 42}) == snapshot(42)
    assert repl.feed_run('f', inputs={'f': 3.14}) == snapshot(3.14)
    assert repl.feed_run('b', inputs={'b': True}) == snapshot(True)
    assert repl.feed_run('lst', inputs={'lst': [1, 2]}) == snapshot([1, 2])


# === Tests for MontyRepl.feed_start(mount=..., os=...) ===


@pytest.fixture
def test_dir() -> Generator[Path, None, None]:
    """Creates a temporary directory with test files for mount-based REPL tests."""
    with tempfile.TemporaryDirectory() as tmpdir:
        p = Path(tmpdir)
        (p / 'hello.txt').write_text('hello world')
        (p / 'subdir').mkdir()
        (p / 'subdir' / 'nested.txt').write_text('nested content')
        yield p


def assert_mount_reusable(md: MountDir) -> None:
    """Assert that a previously used mount was returned to its shared slot."""
    m = pydantic_monty.Monty("from pathlib import Path; Path('/data/subdir/nested.txt').read_text()")
    assert m.run(mount=md) == snapshot('nested content')


def test_feed_start_with_mount_returns_complete(test_dir: Path):
    """feed_start() with mount auto-dispatches OS calls and returns MontyComplete directly."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    repl = pydantic_monty.MontyRepl()
    progress = repl.feed_start("from pathlib import Path; Path('/data/hello.txt').read_text()", mount=md)
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output == snapshot('hello world')
    # REPL remains usable after auto-dispatch completes.
    assert repl.feed_run('1 + 1') == snapshot(2)


def test_feed_start_with_os_callback_returns_complete():
    """feed_start() with os= callback auto-dispatches OS calls to the callback."""

    def os_cb(func: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> bool:
        return True

    repl = pydantic_monty.MontyRepl()
    progress = repl.feed_start("from pathlib import Path; Path('/any/path.txt').exists()", os=os_cb)
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output is True


def test_feed_start_mount_then_external_function(test_dir: Path):
    """OS calls are auto-dispatched, then feed_start returns an external FunctionSnapshot."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
content = Path('/data/hello.txt').read_text()
process(content)
"""
    repl = pydantic_monty.MontyRepl()
    progress = repl.feed_start(code, mount=md)
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    assert progress.is_os_function is False
    assert progress.function_name == snapshot('process')
    assert progress.args == snapshot(('hello world',))
    final = progress.resume({'return_value': 'processed'})
    assert isinstance(final, pydantic_monty.MontyComplete)
    # Subsequent feed_run sees REPL state from the snippet.
    assert repl.feed_run('content') == snapshot('hello world')


def test_feed_start_mount_preserves_repl_state_on_runtime_error(test_dir: Path):
    """When the OS-dispatch chain raises a runtime error, REPL state is preserved."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 42')

    # Reading a path outside the mount raises PermissionError from the sandbox.
    # The feed_start path runs an auto-dispatch loop whose resume fails; the
    # REPL must be rolled back so `x` is still accessible.
    with pytest.raises(pydantic_monty.MontyRuntimeError):
        repl.feed_start("from pathlib import Path; Path('/outside/path.txt').read_text()", mount=md)
    assert repl.feed_run('x') == snapshot(42)
    assert_mount_reusable(md)


def test_feed_start_mount_released_after_resource_error(test_dir: Path):
    """feed_start(mount=...) puts mounts back when a resource limit trips after an OS call."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    repl = pydantic_monty.MontyRepl(limits={'max_memory': 100})
    code = """
from pathlib import Path
Path('/data/hello.txt').read_text()
result = []
for i in range(1000):
    result.append('x' * 100)
len(result)
"""
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        repl.feed_start(code, mount=md)
    assert isinstance(exc_info.value.exception(), MemoryError)
    assert_mount_reusable(md)


def test_feed_start_mount_persists_across_snippets(test_dir: Path):
    """Overlay-mode mount preserves written state across multiple feed_start calls."""
    md = MountDir('/data', str(test_dir), mode='overlay')
    repl = pydantic_monty.MontyRepl()

    # First snippet writes a file via the overlay.
    p1 = repl.feed_start(
        "from pathlib import Path; Path('/data/new.txt').write_text('snippet-one')",
        mount=md,
    )
    assert isinstance(p1, pydantic_monty.MontyComplete)

    # Second snippet reads the file written by the first — proves the mount
    # state (and REPL state) both persisted, and both snippets auto-dispatched.
    p2 = repl.feed_start(
        "from pathlib import Path; Path('/data/new.txt').read_text()",
        mount=md,
    )
    assert isinstance(p2, pydantic_monty.MontyComplete)
    assert p2.output == snapshot('snippet-one')


def test_feed_start_os_not_handled_falls_through():
    """NOT_HANDLED sentinel falls back to Monty's default unhandled behavior."""

    def os_cb(func: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> object:
        return NOT_HANDLED

    repl = pydantic_monty.MontyRepl()
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        repl.feed_start("from pathlib import Path; Path('/any/path.txt').exists()", os=os_cb)
    inner = exc_info.value.exception()
    assert isinstance(inner, PermissionError)
    # REPL state must still be usable after the failed snippet.
    assert repl.feed_run('1 + 1') == snapshot(2)


def test_feed_start_mount_released_after_completion(test_dir: Path):
    """After feed_start auto-dispatches, the mount is put back and usable again."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    repl = pydantic_monty.MontyRepl()
    first = repl.feed_start("from pathlib import Path; Path('/data/hello.txt').read_text()", mount=md)
    assert isinstance(first, pydantic_monty.MontyComplete)

    # Same mount reused by a different Monty instance after feed_start returned.
    m = pydantic_monty.Monty("from pathlib import Path; Path('/data/subdir/nested.txt').read_text()")
    assert m.run(mount=md) == snapshot('nested content')


def test_feed_start_mount_contention_on_os_call_restores_repl_state(test_dir: Path):
    """A mount-take failure during REPL OS auto-dispatch rolls the REPL back."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 42')

    def os_cb(func: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> bool:
        with pytest.raises(ValueError, match='already in use by another run'):
            repl.feed_start("from pathlib import Path; Path('/data/hello.txt').read_text()", mount=md)
        return False

    outer = pydantic_monty.Monty("from pathlib import Path; Path('/outside').exists()")
    result = outer.start(mount=md, os=os_cb)
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output is False
    assert repl.feed_run('x') == snapshot(42)


def test_feed_start_os_handler_invalid_rejected():
    """Non-callable os= is rejected before any REPL state is taken."""
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 1')
    with pytest.raises(TypeError) as exc_info:
        repl.feed_start('x', os=123)  # pyright: ignore[reportArgumentType]
    assert str(exc_info.value) == snapshot("os must be callable, got 'int'")
    # REPL untouched by the validation failure.
    assert repl.feed_run('x') == snapshot(1)


# === Tests for REPL snapshot.resume(mount=..., os=...) ===


def test_repl_function_snapshot_resume_with_mount(test_dir: Path):
    """REPL FunctionSnapshot.resume({'return_value': ...}, mount=...) auto-dispatches OS calls."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
x = fetch()
content = Path('/data/hello.txt').read_text()
result = (x, content)
"""
    repl = pydantic_monty.MontyRepl()
    p1 = repl.feed_start(code)
    assert isinstance(p1, pydantic_monty.FunctionSnapshot)
    assert p1.function_name == snapshot('fetch')

    final = p1.resume({'return_value': 'fetched'}, mount=md)
    assert isinstance(final, pydantic_monty.MontyComplete)
    # State committed — `result` is visible in a subsequent snippet.
    assert repl.feed_run('result') == snapshot(('fetched', 'hello world'))


def test_repl_function_snapshot_resume_with_os_callback():
    """REPL resume() with os= auto-dispatches OS calls via the callback."""

    def os_cb(func: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> bool:
        return True

    repl = pydantic_monty.MontyRepl()
    code = """
from pathlib import Path
x = fetch()
exists = Path('/tmp/x.txt').exists()
result = (x, exists)
"""
    p1 = repl.feed_start(code)
    assert isinstance(p1, pydantic_monty.FunctionSnapshot)
    p1.resume({'return_value': 'fetched'}, os=os_cb)
    assert repl.feed_run('result') == snapshot(('fetched', True))


def test_repl_function_snapshot_resume_yields_next_external(test_dir: Path):
    """REPL resume(..., mount=...) auto-dispatches OS calls then yields next FunctionSnapshot."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
a = first()
c = Path('/data/hello.txt').read_text()
b = second(c)
"""
    repl = pydantic_monty.MontyRepl()
    p1 = repl.feed_start(code)
    assert isinstance(p1, pydantic_monty.FunctionSnapshot)
    assert p1.function_name == snapshot('first')

    p2 = p1.resume({'return_value': 1}, mount=md)
    assert isinstance(p2, pydantic_monty.FunctionSnapshot)
    assert p2.is_os_function is False
    assert p2.function_name == snapshot('second')
    assert p2.args == snapshot(('hello world',))
    p2.resume({'return_value': 2})
    # REPL is usable after completion
    assert repl.feed_run('a') == snapshot(1)


def test_repl_name_lookup_resume_with_mount(test_dir: Path):
    """REPL NameLookupSnapshot.resume(value=..., mount=...) auto-dispatches subsequent OS calls."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
v = my_name
content = Path('/data/hello.txt').read_text()
result = (v, content)
"""
    repl = pydantic_monty.MontyRepl()
    p1 = repl.feed_start(code)
    assert isinstance(p1, pydantic_monty.NameLookupSnapshot)
    assert p1.variable_name == snapshot('my_name')

    final = p1.resume(value=42, mount=md)
    assert isinstance(final, pydantic_monty.MontyComplete)
    assert repl.feed_run('result') == snapshot((42, 'hello world'))


def test_repl_name_lookup_resume_mount_released_after_runtime_error(test_dir: Path):
    """REPL NameLookupSnapshot.resume(..., mount=...) puts mounts back on runtime error."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
value = missing_name
Path('/data/hello.txt').read_text()
1 / 0
"""
    repl = pydantic_monty.MontyRepl()
    progress = repl.feed_start(code)
    assert isinstance(progress, pydantic_monty.NameLookupSnapshot)

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        progress.resume(value=42, mount=md)
    assert isinstance(exc_info.value.exception(), ZeroDivisionError)
    assert repl.feed_run('1 + 1') == snapshot(2)
    assert_mount_reusable(md)


def test_repl_resume_not_handled_with_mount(test_dir: Path):
    """REPL FunctionSnapshot.resume_not_handled(mount=...) auto-dispatches subsequent OS calls."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    code = """
from pathlib import Path
try:
    Path('/outside').exists()
    raised = False
except PermissionError:
    raised = True
content = Path('/data/hello.txt').read_text()
result = (raised, content)
"""
    repl = pydantic_monty.MontyRepl()
    p1 = repl.feed_start(code)
    assert isinstance(p1, pydantic_monty.FunctionSnapshot)
    assert p1.is_os_function is True

    final = p1.resume_not_handled(mount=md)
    assert isinstance(final, pydantic_monty.MontyComplete)
    assert repl.feed_run('result') == snapshot((True, 'hello world'))


def test_repl_resume_mount_preserves_repl_on_error(test_dir: Path):
    """When an OS-dispatch chain during resume fails, REPL state is preserved."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 42')
    code = """
from pathlib import Path
y = fetch()
Path('/outside/path.txt').read_text()
y
"""
    p1 = repl.feed_start(code)
    assert isinstance(p1, pydantic_monty.FunctionSnapshot)

    # The OS call inside the auto-dispatch chain raises PermissionError; the
    # REPL must be rolled back so `x` is still accessible in a later snippet.
    with pytest.raises(pydantic_monty.MontyRuntimeError):
        p1.resume({'return_value': 'fetched'}, mount=md)
    assert repl.feed_run('x') == snapshot(42)
    assert_mount_reusable(md)


def test_repl_resume_mount_released_after_resource_error(test_dir: Path):
    """REPL FunctionSnapshot.resume(..., mount=...) puts mounts back on resource error."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    repl = pydantic_monty.MontyRepl(limits={'max_allocations': 100})
    code = """
from pathlib import Path
value = fetch()
Path('/data/hello.txt').read_text()
result = []
for i in range(10000):
    result.append([i])
len(result)
"""
    progress = repl.feed_start(code)
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        progress.resume({'return_value': 'fetched'}, mount=md)
    assert isinstance(exc_info.value.exception(), MemoryError)
    assert_mount_reusable(md)


def test_repl_resume_mount_put_back_after_completion(test_dir: Path):
    """After REPL resume() with mount completes, the mount can be reused."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    repl = pydantic_monty.MontyRepl()
    code = """
from pathlib import Path
y = fetch()
content = Path('/data/hello.txt').read_text()
result = (y, content)
"""
    p1 = repl.feed_start(code)
    assert isinstance(p1, pydantic_monty.FunctionSnapshot)
    p1.resume({'return_value': 'v'}, mount=md)

    # The same mount should be reusable.
    m = pydantic_monty.Monty("from pathlib import Path; Path('/data/subdir/nested.txt').read_text()")
    assert m.run(mount=md) == snapshot('nested content')


def test_repl_future_snapshot_resume_with_mount(test_dir: Path):
    """REPL FutureSnapshot.resume(..., mount=...) auto-dispatches OS calls and completes."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    repl = pydantic_monty.MontyRepl()
    code = """
result = await fetch()
from pathlib import Path
content = Path('/data/hello.txt').read_text()
pair = (result, content)
"""
    progress = repl.feed_start(code)
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    call_id = progress.call_id

    progress = progress.resume({'future': ...})
    assert isinstance(progress, pydantic_monty.FutureSnapshot)
    final = progress.resume({call_id: {'return_value': 'fetched'}}, mount=md)
    assert isinstance(final, pydantic_monty.MontyComplete)
    assert repl.feed_run('pair') == snapshot(('fetched', 'hello world'))


def test_repl_future_snapshot_resume_mount_released_after_resource_error(test_dir: Path):
    """REPL FutureSnapshot.resume(..., mount=...) puts mounts back on resource error."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    repl = pydantic_monty.MontyRepl(limits={'max_allocations': 100})
    code = """
result = await fetch()
from pathlib import Path
Path('/data/hello.txt').read_text()
values = []
for i in range(10000):
    values.append([i])
len(values)
"""
    progress = repl.feed_start(code)
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    call_id = progress.call_id

    progress = progress.resume({'future': ...})
    assert isinstance(progress, pydantic_monty.FutureSnapshot)
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        progress.resume({call_id: {'return_value': 'fetched'}}, mount=md)
    assert isinstance(exc_info.value.exception(), MemoryError)
    assert_mount_reusable(md)


def test_repl_resume_mount_contention_on_os_call_restores_repl_state(test_dir: Path):
    """A mount-take failure during REPL resume() auto-dispatch rolls the REPL back."""
    md = MountDir('/data', str(test_dir), mode='read-only')
    repl = pydantic_monty.MontyRepl()
    repl.feed_run('x = 42')
    pending = repl.feed_start(
        """
from pathlib import Path
value = fetch()
content = Path('/data/hello.txt').read_text()
result = (value, content)
"""
    )
    assert isinstance(pending, pydantic_monty.FunctionSnapshot)

    def os_cb(func: str, args: tuple[Any, ...], kwargs: dict[str, Any]) -> bool:
        with pytest.raises(ValueError, match='already in use by another run'):
            pending.resume({'return_value': 'fetched'}, mount=md)
        return False

    outer = pydantic_monty.Monty("from pathlib import Path; Path('/outside').exists()")
    result = outer.start(mount=md, os=os_cb)
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output is False
    assert repl.feed_run('x') == snapshot(42)
