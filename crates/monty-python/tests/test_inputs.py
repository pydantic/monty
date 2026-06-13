from __future__ import annotations

from typing import Any

import pytest
from conftest import RunMonty
from inline_snapshot import snapshot

from pydantic_monty import MontyRuntimeError


def test_single_input(monty_run: RunMonty):
    assert monty_run('x', inputs={'x': 42}) == snapshot(42)


def test_multiple_inputs(monty_run: RunMonty):
    assert monty_run('x + y + z', inputs={'x': 1, 'y': 2, 'z': 3}) == snapshot(6)


def test_input_used_in_expression(monty_run: RunMonty):
    assert monty_run('x * 2 + y', inputs={'x': 5, 'y': 3}) == snapshot(13)


def test_input_string(monty_run: RunMonty):
    assert monty_run('greeting + " " + name', inputs={'greeting': 'Hello', 'name': 'World'}) == snapshot('Hello World')


def test_input_list(monty_run: RunMonty):
    assert monty_run('data[0] + data[1]', inputs={'data': [10, 20]}) == snapshot(30)


def test_input_dict(monty_run: RunMonty):
    assert monty_run('config["a"] * config["b"]', inputs={'config': {'a': 3, 'b': 4}}) == snapshot(12)


def test_input_keys_must_be_strings(monty_run: RunMonty):
    bad_inputs: Any = {1: 'x'}
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('x', inputs=bad_inputs)
    assert str(exc_info.value) == snapshot('TypeError: inputs keys must be str')
    assert isinstance(exc_info.value.exception(), TypeError)


def test_missing_input_raises(monty_run: RunMonty):
    # inputs are no longer declared up front, so a missing one is a plain NameError
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('x + y', inputs={'x': 1})
    assert exc_info.value.display(format='type-msg') == snapshot("NameError: name 'y' is not defined")


def test_inputs_order_independent(monty_run: RunMonty):
    # Dict order shouldn't matter
    assert monty_run('a - b', inputs={'b': 3, 'a': 10}) == snapshot(7)


def test_function_param_shadows_input(monty_run: RunMonty):
    """Function parameter should shadow script input with the same name."""
    code = """
def foo(x):
    return x + 1

foo(x * 2)
"""
    # x=5, so foo(x * 2) = foo(10), and inside foo, x is 10 (not 5), so returns 11
    assert monty_run(code, inputs={'x': 5}) == snapshot(11)


def test_function_param_shadows_input_multiple_params(monty_run: RunMonty):
    """Multiple function parameters should all shadow their corresponding inputs."""
    code = """
def add(x, y):
    return x + y

add(x * 10, y * 100)
"""
    # x=2, y=3, so add(20, 300) should return 320
    assert monty_run(code, inputs={'x': 2, 'y': 3}) == snapshot(320)


def test_input_accessible_outside_shadowing_function(monty_run: RunMonty):
    """Script input should still be accessible outside the function that shadows it."""
    code = """
def double(x):
    return x * 2

result = double(10) + x
result
"""
    # double(10) = 20, x (input) = 5, so result = 25
    assert monty_run(code, inputs={'x': 5}) == snapshot(25)


def test_function_param_shadows_input_with_default(monty_run: RunMonty):
    """Function parameter with default should shadow script input when called with arg."""
    code = """
def foo(x=100):
    return x + 1

foo(x * 2)
"""
    # x=5, foo(10), inside foo x=10 (not 5 or 100), returns 11
    assert monty_run(code, inputs={'x': 5}) == snapshot(11)


def test_function_uses_input_directly(monty_run: RunMonty):
    """Function that doesn't shadow should still access the input."""
    code = """
def foo(y):
    return x + y

foo(10)
"""
    # x=5 (input), foo(10) with y=10, returns x + y = 5 + 10 = 15
    assert monty_run(code, inputs={'x': 5}) == snapshot(15)


def test_input_cycle(monty_run: RunMonty):
    x: list[Any] = []
    x.append(x)
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('x', inputs={'x': x})
    assert str(exc_info.value) == snapshot('RuntimeError: Max input depth exceeded')


def test_input_deep(monty_run: RunMonty):
    x: list[Any] = [1]
    for _ in range(300):
        x = [x]
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('x', inputs={'x': x})
    assert str(exc_info.value) == snapshot('RuntimeError: Max input depth exceeded')


def test_output_deep(monty_run: RunMonty):
    # Sandbox code that iteratively builds a deeply nested list bypasses the
    # Python-level recursion limit (the `for` loop never pushes a call frame).
    # Result values deeper than the wire protocol's nesting bound are rejected
    # by the worker with a clean, session-preserving error.
    code = """
x = [1]
for _ in range(300):
    x = [x]
x
"""
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run(code)
    assert str(exc_info.value) == snapshot('RuntimeError: Max output depth exceeded')


def test_empty_inputs(monty_run: RunMonty):
    assert monty_run('1 + 1', inputs={}) == snapshot(2)
