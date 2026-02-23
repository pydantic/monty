from inline_snapshot import snapshot

import pydantic_monty


def test_simple_expression():
    m = pydantic_monty.Monty('1 + 2')
    assert m.run() == snapshot(3)


def test_arithmetic():
    m = pydantic_monty.Monty('10 * 5 - 3')
    assert m.run() == snapshot(47)


def test_string_concatenation():
    m = pydantic_monty.Monty('"hello" + " " + "world"')
    assert m.run() == snapshot('hello world')


def test_multiple_runs_same_instance():
    m = pydantic_monty.Monty('x * 2', inputs=['x'])
    assert m.run(inputs={'x': 5}) == snapshot(10)
    assert m.run(inputs={'x': 10}) == snapshot(20)
    assert m.run(inputs={'x': -3}) == snapshot(-6)


def test_repr_no_inputs():
    m = pydantic_monty.Monty('1 + 1')
    assert repr(m) == snapshot("Monty(<1 line of code>, script_name='main.py')")


def test_repr_with_inputs():
    m = pydantic_monty.Monty('x', inputs=['x', 'y'])
    assert repr(m) == snapshot('Monty(<1 line of code>, script_name=\'main.py\', inputs=["x", "y"])')


def test_repr_with_external_functions():
    m = pydantic_monty.Monty('foo()', external_functions=['foo'])
    assert repr(m) == snapshot('Monty(<1 line of code>, script_name=\'main.py\', external_functions=["foo"])')


def test_repr_with_inputs_and_external_functions():
    m = pydantic_monty.Monty('foo(x)', inputs=['x'], external_functions=['foo'])
    assert repr(m) == snapshot(
        'Monty(<1 line of code>, script_name=\'main.py\', inputs=["x"], external_functions=["foo"])'
    )


def test_multiline_code():
    code = """
x = 1
y = 2
x + y
"""
    m = pydantic_monty.Monty(code)
    assert m.run() == snapshot(3)


def test_function_definition_and_call():
    code = """
def add(a, b):
    return a + b

add(3, 4)
"""
    m = pydantic_monty.Monty(code)
    assert m.run() == snapshot(7)
