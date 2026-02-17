import pytest
from inline_snapshot import snapshot

import pydantic_monty


def test_single_input():
    m = pydantic_monty.Monty('x', inputs=['x'])
    assert m.run(inputs={'x': 42}) == snapshot(42)


def test_multiple_inputs():
    m = pydantic_monty.Monty('x + y + z', inputs=['x', 'y', 'z'])
    assert m.run(inputs={'x': 1, 'y': 2, 'z': 3}) == snapshot(6)


def test_input_used_in_expression():
    m = pydantic_monty.Monty('x * 2 + y', inputs=['x', 'y'])
    assert m.run(inputs={'x': 5, 'y': 3}) == snapshot(13)


def test_input_string():
    m = pydantic_monty.Monty('greeting + " " + name', inputs=['greeting', 'name'])
    assert m.run(inputs={'greeting': 'Hello', 'name': 'World'}) == snapshot('Hello World')


def test_input_list():
    m = pydantic_monty.Monty('data[0] + data[1]', inputs=['data'])
    assert m.run(inputs={'data': [10, 20]}) == snapshot(30)


def test_input_dict():
    m = pydantic_monty.Monty('config["a"] * config["b"]', inputs=['config'])
    assert m.run(inputs={'config': {'a': 3, 'b': 4}}) == snapshot(12)


def test_missing_input_raises():
    m = pydantic_monty.Monty('x + y', inputs=['x', 'y'])
    with pytest.raises(KeyError, match="Missing required input: 'y'"):
        m.run(inputs={'x': 1})


def test_all_inputs_missing_raises():
    m = pydantic_monty.Monty('x', inputs=['x'])
    with pytest.raises(TypeError, match='Missing required inputs'):
        m.run()


def test_no_inputs_declared_but_provided_raises():
    m = pydantic_monty.Monty('1 + 1')
    with pytest.raises(TypeError, match='No input variables declared but inputs dict was provided'):
        m.run(inputs={'x': 1})
        with pytest.raises(TypeError, match='No input variables declared but inputs dict was provided'):
            m.run(inputs={})


def test_inputs_order_independent():
    m = pydantic_monty.Monty('a - b', inputs=['a', 'b'])
    # Dict order shouldn't matter
    assert m.run(inputs={'b': 3, 'a': 10}) == snapshot(7)


def test_function_param_shadows_input():
    """Function parameter should shadow script input with the same name."""
    code = """
def foo(x):
    return x + 1

foo(x * 2)
"""
    m = pydantic_monty.Monty(code, inputs=['x'])
    # x=5, so foo(x * 2) = foo(10), and inside foo, x is 10 (not 5), so returns 11
    assert m.run(inputs={'x': 5}) == snapshot(11)


def test_function_param_shadows_input_multiple_params():
    """Multiple function parameters should all shadow their corresponding inputs."""
    code = """
def add(x, y):
    return x + y

add(x * 10, y * 100)
"""
    m = pydantic_monty.Monty(code, inputs=['x', 'y'])
    # x=2, y=3, so add(20, 300) should return 320
    assert m.run(inputs={'x': 2, 'y': 3}) == snapshot(320)


def test_input_accessible_outside_shadowing_function():
    """Script input should still be accessible outside the function that shadows it."""
    code = """
def double(x):
    return x * 2

result = double(10) + x
result
"""
    m = pydantic_monty.Monty(code, inputs=['x'])
    # double(10) = 20, x (input) = 5, so result = 25
    assert m.run(inputs={'x': 5}) == snapshot(25)


def test_function_param_shadows_input_with_default():
    """Function parameter with default should shadow script input when called with arg."""
    code = """
def foo(x=100):
    return x + 1

foo(x * 2)
"""
    m = pydantic_monty.Monty(code, inputs=['x'])
    # x=5, foo(10), inside foo x=10 (not 5 or 100), returns 11
    assert m.run(inputs={'x': 5}) == snapshot(11)


def test_function_uses_input_directly():
    """Function that doesn't shadow should still access the input."""
    code = """
def foo(y):
    return x + y

foo(10)
"""
    m = pydantic_monty.Monty(code, inputs=['x'])
    # x=5 (input), foo(10) with y=10, returns x + y = 5 + 10 = 15
    assert m.run(inputs={'x': 5}) == snapshot(15)
