"""Session-as-REPL tests: state persists across `feed_run` calls within a checkout."""

from __future__ import annotations

from typing import Callable, Literal

import pytest
from conftest import RunMonty
from inline_snapshot import snapshot

from pydantic_monty import Monty, MontyRuntimeError, MontySession, MontySyntaxError

PrintCallback = Callable[[Literal['stdout', 'stderr'], str], None]


def make_print_collector() -> tuple[list[str], PrintCallback]:
    """Create a print callback that collects output into a list."""
    output: list[str] = []

    def callback(stream: Literal['stdout', 'stderr'], text: str) -> None:
        assert stream == 'stdout'
        output.append(text)

    return output, callback


# === Basic feed_run behavior ===


def test_feed_run_expression_returns_value(session: MontySession):
    assert session.feed_run('1 + 2') == snapshot(3)


def test_feed_run_assignment_returns_none(session: MontySession):
    assert session.feed_run('x = 42') == snapshot(None)


def test_feed_run_empty_string_returns_none(session: MontySession):
    assert session.feed_run('') == snapshot(None)


def test_feed_run_none_literal(session: MontySession):
    assert session.feed_run('None') is None


# === State persistence across feeds ===


def test_variable_persists_across_feeds(session: MontySession):
    session.feed_run('x = 10')
    assert session.feed_run('x') == snapshot(10)


def test_incremental_mutation(session: MontySession):
    session.feed_run('counter = 0')
    session.feed_run('counter = counter + 1')
    session.feed_run('counter = counter + 1')
    assert session.feed_run('counter') == snapshot(2)


def test_multiple_variables(session: MontySession):
    session.feed_run('x = 10')
    session.feed_run('y = 20')
    assert session.feed_run('x + y') == snapshot(30)


def test_function_defined_then_called(session: MontySession):
    session.feed_run('def double(n):\n    return n * 2')
    assert session.feed_run('double(21)') == snapshot(42)


def test_function_uses_previously_defined_variable(session: MontySession):
    session.feed_run('factor = 3')
    session.feed_run('def multiply(n):\n    return n * factor')
    assert session.feed_run('multiply(7)') == snapshot(21)


def test_closure_persists_across_feeds(session: MontySession):
    session.feed_run("""\
def make_counter():
    count = 0
    def increment():
        nonlocal count
        count += 1
        return count
    return increment
""")
    session.feed_run('counter = make_counter()')
    assert session.feed_run('counter()') == snapshot(1)
    assert session.feed_run('counter()') == snapshot(2)


def test_list_mutation_persists(session: MontySession):
    session.feed_run('items = [1, 2, 3]')
    session.feed_run('items.append(4)')
    assert session.feed_run('len(items)') == snapshot(4)
    assert session.feed_run('items') == snapshot([1, 2, 3, 4])


def test_dict_mutation_persists(session: MontySession):
    session.feed_run("data = {'a': 1}")
    session.feed_run("data['b'] = 2")
    assert session.feed_run('len(data)') == snapshot(2)
    assert session.feed_run("data['b']") == snapshot(2)


def test_variable_reassignment(session: MontySession):
    session.feed_run('x = "hello"')
    assert session.feed_run('x') == snapshot('hello')
    session.feed_run('x = 42')
    assert session.feed_run('x') == snapshot(42)


# === Multi-statement snippets ===


def test_multi_statement_snippet(session: MontySession):
    session.feed_run('a = 1\nb = 2\nc = a + b')
    assert session.feed_run('c') == snapshot(3)


def test_loop_in_snippet(session: MontySession):
    session.feed_run('total = 0\nfor i in range(5):\n    total = total + i')
    assert session.feed_run('total') == snapshot(10)


def test_if_else_in_snippet(session: MontySession):
    session.feed_run('x = 10')
    session.feed_run('result = "big" if x > 5 else "small"')
    assert session.feed_run('result') == snapshot('big')


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
def test_feed_run_return_types(monty_run: RunMonty, code: str, expected: object):
    assert monty_run(code) == expected


# === Error handling ===


def test_syntax_error(session: MontySession):
    # syntax errors surface at feed_run time, not construction
    with pytest.raises(MontySyntaxError):
        session.feed_run('def')


def test_syntax_error_preserves_session(session: MontySession):
    session.feed_run('x = 5')
    with pytest.raises(MontySyntaxError):
        session.feed_run('def foo(:')
    assert session.feed_run('x') == snapshot(5)


def test_runtime_error_preserves_state(session: MontySession):
    """A runtime error should not destroy previously defined state."""
    session.feed_run('x = 42')
    with pytest.raises(MontyRuntimeError):
        session.feed_run('1 / 0')
    # x should still be accessible after the error
    assert session.feed_run('x') == snapshot(42)


def test_name_error(session: MontySession):
    with pytest.raises(MontyRuntimeError) as exc_info:
        session.feed_run('undefined_var')
    assert isinstance(exc_info.value.exception(), NameError)


def test_type_error(session: MontySession):
    with pytest.raises(MontyRuntimeError) as exc_info:
        session.feed_run('"hello" + 1')
    assert isinstance(exc_info.value.exception(), TypeError)


def test_zero_division_error(session: MontySession):
    with pytest.raises(MontyRuntimeError) as exc_info:
        session.feed_run('1 / 0')
    assert isinstance(exc_info.value.exception(), ZeroDivisionError)


def test_index_error(session: MontySession):
    with pytest.raises(MontyRuntimeError) as exc_info:
        session.feed_run('[1, 2][10]')
    assert isinstance(exc_info.value.exception(), IndexError)


def test_key_error(session: MontySession):
    with pytest.raises(MontyRuntimeError) as exc_info:
        session.feed_run("{'a': 1}['b']")
    assert isinstance(exc_info.value.exception(), KeyError)


def test_multiple_errors_dont_corrupt_state(session: MontySession):
    session.feed_run('x = 1')
    with pytest.raises(MontyRuntimeError):
        session.feed_run('1 / 0')
    session.feed_run('x = x + 1')
    with pytest.raises(MontyRuntimeError):
        session.feed_run('undefined_name')
    assert session.feed_run('x') == snapshot(2)


def test_cross_snippet_traceback_resolves_against_defining_snippet(session: MontySession):
    # REPL tracebacks must resolve each frame against the source text of the
    # snippet that actually produced the CodeRange byte offsets — not the
    # snippet that happens to be executing when the exception fires. The
    # function below is defined in snippet 0 (`<python-input-0>`), called
    # from snippet 1 (`<python-input-1>`); the raise-site frame needs to
    # point back at snippet 0's source for line/column/source_line.
    session.feed_run("def f():\n    raise ValueError('boom')")
    with pytest.raises(MontyRuntimeError) as exc_info:
        session.feed_run('f()')
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


def test_input_invalid_identifier(session: MontySession):
    with pytest.raises(MontySyntaxError) as exc_info:
        session.feed_run('x', inputs={'foo.bar': 42})
    assert str(exc_info.value) == snapshot("Input name 'foo.bar' not a valid identifier")


# === Print callback ===


def test_print_callback_on_feed(session: MontySession):
    output, callback = make_print_collector()
    session.feed_run('print("hello")', print_callback=callback)
    assert ''.join(output) == snapshot('hello\n')


def test_print_callback_across_feeds(session: MontySession):
    output, callback = make_print_collector()
    session.feed_run('print("first")', print_callback=callback)
    session.feed_run('print("second")', print_callback=callback)
    assert ''.join(output) == snapshot('first\nsecond\n')


# === Resource limits (checkout-level) ===


def test_checkout_with_limits(monty_run: RunMonty):
    assert monty_run('1 + 1', limits={'max_duration_secs': 5.0}) == snapshot(2)


def test_infinite_loop_with_limits(monty_run: RunMonty):
    with pytest.raises(MontyRuntimeError):
        monty_run('while True:\n    pass', limits={'max_duration_secs': 0.5})


# === External functions ===


def test_external_function_basic(session: MontySession):
    def add(a: int, b: int) -> int:
        return a + b

    assert session.feed_run('result = add(3, 4)', external_lookup={'add': add}) == snapshot(None)
    assert session.feed_run('result') == snapshot(7)


def test_external_function_return_value(session: MontySession):
    def greet(name: str) -> str:
        return f'hello {name}'

    assert session.feed_run('greet("world")', external_lookup={'greet': greet}) == snapshot('hello world')


def test_external_function_called_multiple_times(session: MontySession):
    call_count = 0

    def counter():
        nonlocal call_count
        call_count += 1
        return call_count

    ext = {'counter': counter}
    assert session.feed_run('counter()', external_lookup=ext) == snapshot(1)
    assert session.feed_run('counter()', external_lookup=ext) == snapshot(2)
    assert call_count == 2


def test_external_function_persists_state_across_feeds(session: MontySession):
    def double(x: int) -> int:
        return x * 2

    session.feed_run('x = 5')
    assert session.feed_run('double(x)', external_lookup={'double': double}) == snapshot(10)


def test_external_function_exception_becomes_runtime_error(session: MontySession):
    def fail():
        raise ValueError('external failure')

    with pytest.raises(MontyRuntimeError) as exc_info:
        session.feed_run('fail()', external_lookup={'fail': fail})
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert str(inner) == snapshot('external failure')


def test_external_function_error_preserves_session_state(session: MontySession):
    def fail():
        raise ValueError('boom')

    session.feed_run('x = 42')
    with pytest.raises(MontyRuntimeError):
        session.feed_run('fail()', external_lookup={'fail': fail})
    # session state should be preserved after error
    assert session.feed_run('x') == snapshot(42)


def test_external_function_undefined_raises_name_error(session: MontySession):
    """Calling a name that's not in external_lookup raises NameError."""
    with pytest.raises(MontyRuntimeError) as exc_info:
        session.feed_run('unknown()', external_lookup={'known': lambda: 1})
    assert isinstance(exc_info.value.exception(), NameError)


def test_external_function_with_print_callback(session: MontySession):
    output, callback = make_print_collector()
    ext = {'get_msg': lambda: 'from external'}
    session.feed_run('x = get_msg()\nprint(x)', external_lookup=ext, print_callback=callback)
    assert ''.join(output) == snapshot('from external\n')


def test_external_function_with_kwargs(session: MontySession):
    def greet(name: str, greeting: str = 'hello') -> str:
        return f'{greeting} {name}'

    assert session.feed_run("greet('world', greeting='hi')", external_lookup={'greet': greet}) == snapshot('hi world')


# === Inputs ===


def test_inputs_basic(session: MontySession):
    assert session.feed_run('x + 1', inputs={'x': 10}) == snapshot(11)


def test_inputs_used_in_same_snippet(session: MontySession):
    session.feed_run('y = x + 1', inputs={'x': 42})
    assert session.feed_run('y') == snapshot(43)


def test_inputs_multiple_values(session: MontySession):
    assert session.feed_run('a + b', inputs={'a': 3, 'b': 7}) == snapshot(10)


def test_inputs_override_existing_variable(session: MontySession):
    session.feed_run('x = 1')
    assert session.feed_run('x', inputs={'x': 99}) == snapshot(99)


def test_inputs_with_external_lookup(session: MontySession):
    def double(n: int) -> int:
        return n * 2

    assert session.feed_run('double(x)', inputs={'x': 5}, external_lookup={'double': double}) == snapshot(10)


def test_inputs_various_types(session: MontySession):
    assert session.feed_run('s', inputs={'s': 'hello'}) == snapshot('hello')
    assert session.feed_run('n', inputs={'n': 42}) == snapshot(42)
    assert session.feed_run('f', inputs={'f': 3.14}) == snapshot(3.14)
    assert session.feed_run('b', inputs={'b': True}) == snapshot(True)
    assert session.feed_run('lst', inputs={'lst': [1, 2]}) == snapshot([1, 2])


# === Sessions are isolated ===


def test_new_checkout_has_fresh_state(pool: Monty):
    with pool.checkout() as session:
        session.feed_run('marker = 123')
    with pool.checkout() as session:
        with pytest.raises(MontyRuntimeError) as exc_info:
            session.feed_run('marker')
        assert isinstance(exc_info.value.exception(), NameError)
