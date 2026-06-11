from __future__ import annotations

from conftest import RunMonty
from inline_snapshot import snapshot


def test_simple_expression(monty_run: RunMonty):
    assert monty_run('1 + 2') == snapshot(3)


def test_arithmetic(monty_run: RunMonty):
    assert monty_run('10 * 5 - 3') == snapshot(47)


def test_string_concatenation(monty_run: RunMonty):
    assert monty_run('"hello" + " " + "world"') == snapshot('hello world')


def test_multiple_runs_same_code(monty_run: RunMonty):
    assert monty_run('x * 2', inputs={'x': 5}) == snapshot(10)
    assert monty_run('x * 2', inputs={'x': 10}) == snapshot(20)
    assert monty_run('x * 2', inputs={'x': -3}) == snapshot(-6)


def test_multiline_code(monty_run: RunMonty):
    code = """
x = 1
y = 2
x + y
"""
    assert monty_run(code) == snapshot(3)


def test_function_definition_and_call(monty_run: RunMonty):
    code = """
def add(a, b):
    return a + b

add(3, 4)
"""
    assert monty_run(code) == snapshot(7)
