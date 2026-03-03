from typing import Callable, Literal

import pytest
from inline_snapshot import snapshot

import pydantic_monty

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


# === Basic feed behavior ===


def test_feed_expression_returns_value():
    repl = pydantic_monty.MontyRepl()
    assert repl.feed('1 + 2') == snapshot(3)


def test_feed_assignment_returns_none():
    repl = pydantic_monty.MontyRepl()
    assert repl.feed('x = 42') == snapshot(None)


def test_feed_empty_string_returns_none():
    repl = pydantic_monty.MontyRepl()
    assert repl.feed('') == snapshot(None)


def test_feed_none_literal():
    repl = pydantic_monty.MontyRepl()
    assert repl.feed('None') is None


# === State persistence across feeds ===


def test_variable_persists_across_feeds():
    repl = pydantic_monty.MontyRepl()
    repl.feed('x = 10')
    assert repl.feed('x') == snapshot(10)


def test_incremental_mutation():
    repl = pydantic_monty.MontyRepl()
    repl.feed('counter = 0')
    repl.feed('counter = counter + 1')
    repl.feed('counter = counter + 1')
    assert repl.feed('counter') == snapshot(2)


def test_multiple_variables():
    repl = pydantic_monty.MontyRepl()
    repl.feed('x = 10')
    repl.feed('y = 20')
    assert repl.feed('x + y') == snapshot(30)


def test_function_defined_then_called():
    repl = pydantic_monty.MontyRepl()
    repl.feed('def double(n):\n    return n * 2')
    assert repl.feed('double(21)') == snapshot(42)


def test_function_uses_previously_defined_variable():
    repl = pydantic_monty.MontyRepl()
    repl.feed('factor = 3')
    repl.feed('def multiply(n):\n    return n * factor')
    assert repl.feed('multiply(7)') == snapshot(21)


def test_list_mutation_persists():
    repl = pydantic_monty.MontyRepl()
    repl.feed('items = [1, 2, 3]')
    repl.feed('items.append(4)')
    assert repl.feed('len(items)') == snapshot(4)
    assert repl.feed('items') == snapshot([1, 2, 3, 4])


def test_dict_mutation_persists():
    repl = pydantic_monty.MontyRepl()
    repl.feed("data = {'a': 1}")
    repl.feed("data['b'] = 2")
    assert repl.feed('len(data)') == snapshot(2)
    assert repl.feed("data['b']") == snapshot(2)


def test_variable_reassignment():
    repl = pydantic_monty.MontyRepl()
    repl.feed('x = "hello"')
    assert repl.feed('x') == snapshot('hello')
    repl.feed('x = 42')
    assert repl.feed('x') == snapshot(42)


# === Multi-statement snippets ===


def test_multi_statement_snippet():
    repl = pydantic_monty.MontyRepl()
    repl.feed('a = 1\nb = 2\nc = a + b')
    assert repl.feed('c') == snapshot(3)


def test_loop_in_snippet():
    repl = pydantic_monty.MontyRepl()
    repl.feed('total = 0\nfor i in range(5):\n    total = total + i')
    assert repl.feed('total') == snapshot(10)


def test_if_else_in_snippet():
    repl = pydantic_monty.MontyRepl()
    repl.feed('x = 10')
    repl.feed('result = "big" if x > 5 else "small"')
    assert repl.feed('result') == snapshot('big')


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
def test_feed_return_types(code: str, expected: object):
    repl = pydantic_monty.MontyRepl()
    assert repl.feed(code) == expected


# === Error handling ===


def test_syntax_error():
    repl = pydantic_monty.MontyRepl()
    with pytest.raises(pydantic_monty.MontySyntaxError):
        repl.feed('def')


def test_runtime_error_preserves_state():
    """A runtime error should not destroy previously defined state."""
    repl = pydantic_monty.MontyRepl()
    repl.feed('x = 42')
    with pytest.raises(pydantic_monty.MontyRuntimeError):
        repl.feed('1 / 0')
    # x should still be accessible after the error
    assert repl.feed('x') == snapshot(42)


def test_name_error():
    repl = pydantic_monty.MontyRepl()
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        repl.feed('undefined_var')
    inner = exc_info.value.exception()
    assert isinstance(inner, NameError)


def test_type_error():
    repl = pydantic_monty.MontyRepl()
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        repl.feed('"hello" + 1')
    inner = exc_info.value.exception()
    assert isinstance(inner, TypeError)


def test_zero_division_error():
    repl = pydantic_monty.MontyRepl()
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        repl.feed('1 / 0')
    inner = exc_info.value.exception()
    assert isinstance(inner, ZeroDivisionError)


def test_index_error():
    repl = pydantic_monty.MontyRepl()
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        repl.feed('[1, 2][10]')
    inner = exc_info.value.exception()
    assert isinstance(inner, IndexError)


def test_key_error():
    repl = pydantic_monty.MontyRepl()
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        repl.feed("{'a': 1}['b']")
    inner = exc_info.value.exception()
    assert isinstance(inner, KeyError)


def test_multiple_errors_dont_corrupt_state():
    repl = pydantic_monty.MontyRepl()
    repl.feed('x = 1')
    with pytest.raises(pydantic_monty.MontyRuntimeError):
        repl.feed('1 / 0')
    repl.feed('x = x + 1')
    with pytest.raises(pydantic_monty.MontyRuntimeError):
        repl.feed('undefined_name')
    assert repl.feed('x') == snapshot(2)


# === Print callback ===


def test_print_callback_on_constructor():
    output, callback = make_print_collector()
    repl = pydantic_monty.MontyRepl(print_callback=callback)
    repl.feed('print("hello")')
    assert ''.join(output) == snapshot('hello\n')


def test_print_callback_on_feed():
    repl = pydantic_monty.MontyRepl()
    output, callback = make_print_collector()
    repl.feed('print("hello")', print_callback=callback)
    assert ''.join(output) == snapshot('hello\n')


def test_print_callback_per_feed_overrides_constructor():
    ctor_output, ctor_callback = make_print_collector()
    repl = pydantic_monty.MontyRepl(print_callback=ctor_callback)

    feed_output, feed_callback = make_print_collector()
    repl.feed('print("routed")', print_callback=feed_callback)

    assert ''.join(ctor_output) == snapshot('')
    assert ''.join(feed_output) == snapshot('routed\n')


def test_print_callback_persists_across_feeds():
    output, callback = make_print_collector()
    repl = pydantic_monty.MontyRepl(print_callback=callback)
    repl.feed('print("first")')
    repl.feed('print("second")')
    assert ''.join(output) == snapshot('first\nsecond\n')


# === Resource limits ===


def test_construction_with_limits():
    limits = pydantic_monty.ResourceLimits(max_duration_secs=5.0)
    repl = pydantic_monty.MontyRepl(limits=limits)
    assert repl.feed('1 + 1') == snapshot(2)


def test_infinite_loop_with_limits():
    limits = pydantic_monty.ResourceLimits(max_duration_secs=0.5)
    repl = pydantic_monty.MontyRepl(limits=limits)
    with pytest.raises(pydantic_monty.MontyRuntimeError):
        repl.feed('while True:\n    pass')


# === Serialization ===


def test_dump_load_roundtrip():
    repl = pydantic_monty.MontyRepl()
    repl.feed('x = 40')
    repl.feed('x = x + 1')

    serialized = repl.dump()
    assert isinstance(serialized, bytes)

    loaded = pydantic_monty.MontyRepl.load(serialized)
    assert loaded.feed('x + 1') == snapshot(42)


def test_dump_load_preserves_functions():
    repl = pydantic_monty.MontyRepl()
    repl.feed('def greet(name):\n    return "hello " + name')

    loaded = pydantic_monty.MontyRepl.load(repl.dump())
    assert loaded.feed('greet("world")') == snapshot('hello world')


def test_dump_load_preserves_script_name():
    repl = pydantic_monty.MontyRepl(script_name='custom.py')
    loaded = pydantic_monty.MontyRepl.load(repl.dump())
    assert loaded.script_name == snapshot('custom.py')


def test_load_with_print_callback():
    repl = pydantic_monty.MontyRepl()
    repl.feed('x = 1')

    output, callback = make_print_collector()
    loaded = pydantic_monty.MontyRepl.load(repl.dump(), print_callback=callback)
    loaded.feed('print(x)')
    assert ''.join(output) == snapshot('1\n')


def test_load_invalid_data():
    with pytest.raises(ValueError):
        pydantic_monty.MontyRepl.load(b'invalid data')
