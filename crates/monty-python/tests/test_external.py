from typing import Any

import pytest
from inline_snapshot import snapshot

import pydantic_monty


def test_external_function_no_args():
    m = pydantic_monty.Monty('noop()')

    def noop(*args: Any, **kwargs: Any) -> str:
        assert args == snapshot(())
        assert kwargs == snapshot({})
        return 'called'

    assert m.run(external_functions={'noop': noop}) == snapshot('called')


def test_external_function_positional_args():
    m = pydantic_monty.Monty('func(1, 2, 3)')

    def func(*args: Any, **kwargs: Any) -> str:
        assert args == snapshot((1, 2, 3))
        assert kwargs == snapshot({})
        return 'ok'

    assert m.run(external_functions={'func': func}) == snapshot('ok')


def test_external_function_kwargs_only():
    m = pydantic_monty.Monty('func(a=1, b="two")')

    def func(*args: Any, **kwargs: Any) -> str:
        assert args == snapshot(())
        assert kwargs == snapshot({'a': 1, 'b': 'two'})
        return 'ok'

    assert m.run(external_functions={'func': func}) == snapshot('ok')


def test_external_function_mixed_args_kwargs():
    m = pydantic_monty.Monty('func(1, 2, x="hello", y=True)')

    def func(*args: Any, **kwargs: Any) -> str:
        assert args == snapshot((1, 2))
        assert kwargs == snapshot({'x': 'hello', 'y': True})
        return 'ok'

    assert m.run(external_functions={'func': func}) == snapshot('ok')


def test_external_function_complex_types():
    m = pydantic_monty.Monty('func([1, 2], {"key": "value"})')

    def func(*args: Any, **kwargs: Any) -> str:
        assert args == snapshot(([1, 2], {'key': 'value'}))
        assert kwargs == snapshot({})
        return 'ok'

    assert m.run(external_functions={'func': func}) == snapshot('ok')


def test_external_function_returns_none():
    m = pydantic_monty.Monty('do_nothing()')

    def do_nothing(*args: Any, **kwargs: Any) -> None:
        assert args == snapshot(())
        assert kwargs == snapshot({})

    assert m.run(external_functions={'do_nothing': do_nothing}) is None


def test_external_function_returns_complex_type():
    m = pydantic_monty.Monty('get_data()')

    def get_data(*args: Any, **kwargs: Any) -> dict[str, Any]:
        return {'a': [1, 2, 3], 'b': {'nested': True}}

    result = m.run(external_functions={'get_data': get_data})
    assert result == snapshot({'a': [1, 2, 3], 'b': {'nested': True}})


def test_multiple_external_functions():
    m = pydantic_monty.Monty('add(1, 2) + mul(3, 4)')

    def add(*args: Any, **kwargs: Any) -> int:
        assert args == snapshot((1, 2))
        assert kwargs == snapshot({})
        return args[0] + args[1]

    def mul(*args: Any, **kwargs: Any) -> int:
        assert args == snapshot((3, 4))
        assert kwargs == snapshot({})
        return args[0] * args[1]

    result = m.run(external_functions={'add': add, 'mul': mul})
    assert result == snapshot(15)  # 3 + 12


def test_external_function_called_multiple_times():
    m = pydantic_monty.Monty('counter() + counter() + counter()')

    call_count = 0

    def counter(*args: Any, **kwargs: Any) -> int:
        nonlocal call_count
        assert args == snapshot(())
        assert kwargs == snapshot({})
        call_count += 1
        return call_count

    result = m.run(external_functions={'counter': counter})
    assert result == snapshot(6)  # 1 + 2 + 3
    assert call_count == snapshot(3)


def test_external_function_with_input():
    m = pydantic_monty.Monty('process(x)', inputs=['x'])

    def process(*args: Any, **kwargs: Any) -> int:
        assert args == snapshot((5,))
        assert kwargs == snapshot({})
        return args[0] * 10

    assert m.run(inputs={'x': 5}, external_functions={'process': process}) == snapshot(50)


def test_external_function_not_provided_raises_name_error():
    """Calling an unknown function without external_functions raises NameError."""
    m = pydantic_monty.Monty('missing()')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run()
    inner = exc_info.value.exception()
    assert type(inner) is NameError
    assert str(inner) == snapshot("name 'missing' is not defined")


def test_undeclared_function_raises_name_error():
    m = pydantic_monty.Monty('unknown_func()')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run()
    inner = exc_info.value.exception()
    assert type(inner) is NameError
    assert str(inner) == snapshot("name 'unknown_func' is not defined")


def test_external_function_raises_exception():
    """Test that exceptions from external functions propagate to the caller."""
    m = pydantic_monty.Monty('fail()')

    def fail(*args: Any, **kwargs: Any) -> None:
        raise ValueError('intentional error')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(external_functions={'fail': fail})
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert inner.args[0] == snapshot('intentional error')


def test_external_function_wrong_name_raises():
    """Test that calling a function not in external_functions raises NameError."""
    m = pydantic_monty.Monty('foo()')

    def bar(*args: Any, **kwargs: Any) -> int:
        return 1

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(external_functions={'bar': bar})
    inner = exc_info.value.exception()
    assert type(inner) is NameError
    assert str(inner) == snapshot("name 'foo' is not defined")


def test_external_function_exception_caught_by_try_except():
    """Test that exceptions from external functions can be caught by try/except."""
    code = """
try:
    fail()
except ValueError:
    caught = True
caught
"""
    m = pydantic_monty.Monty(code)

    def fail(*args: Any, **kwargs: Any) -> None:
        raise ValueError('caught error')

    result = m.run(external_functions={'fail': fail})
    assert result == snapshot(True)


def test_external_function_exception_type_preserved():
    """Test that various exception types are correctly preserved."""
    m = pydantic_monty.Monty('fail()')

    def fail_type_error(*args: Any, **kwargs: Any) -> None:
        raise TypeError('type error message')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(external_functions={'fail': fail_type_error})
    inner = exc_info.value.exception()
    assert isinstance(inner, TypeError)
    assert inner.args[0] == snapshot('type error message')


@pytest.mark.parametrize(
    'exception_class,exception_name',
    [
        # ArithmeticError hierarchy
        (ZeroDivisionError, 'ZeroDivisionError'),
        (OverflowError, 'OverflowError'),
        (ArithmeticError, 'ArithmeticError'),
        # RuntimeError hierarchy
        (NotImplementedError, 'NotImplementedError'),
        (RecursionError, 'RecursionError'),
        (RuntimeError, 'RuntimeError'),
        # LookupError hierarchy
        (KeyError, 'KeyError'),
        (IndexError, 'IndexError'),
        (LookupError, 'LookupError'),
        # Other exceptions
        (ValueError, 'ValueError'),
        (TypeError, 'TypeError'),
        (AttributeError, 'AttributeError'),
        (NameError, 'NameError'),
        (AssertionError, 'AssertionError'),
    ],
)
def test_external_function_exception_hierarchy(exception_class: type[BaseException], exception_name: str):
    """Test that exception types in hierarchies are correctly preserved."""
    # Test that exception propagates with correct type
    m = pydantic_monty.Monty('fail()')

    def fail(*args: Any, **kwargs: Any) -> None:
        raise exception_class('test message')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(external_functions={'fail': fail})
    inner = exc_info.value.exception()
    assert isinstance(inner, exception_class)


@pytest.mark.parametrize(
    'exception_class,parent_class,expected_result',
    [
        # ArithmeticError hierarchy
        (ZeroDivisionError, ArithmeticError, 'child'),
        (OverflowError, ArithmeticError, 'child'),
        # RuntimeError hierarchy
        (NotImplementedError, RuntimeError, 'child'),
        (RecursionError, RuntimeError, 'child'),
        # LookupError hierarchy
        (KeyError, LookupError, 'child'),
        (IndexError, LookupError, 'child'),
    ],
)
def test_external_function_exception_caught_by_parent(
    exception_class: type[BaseException], parent_class: type[BaseException], expected_result: str
):
    """Test that child exceptions can be caught by parent except handlers."""
    code = f"""
try:
    fail()
except {parent_class.__name__}:
    caught = 'parent'
except {exception_class.__name__}:
    caught = 'child'
caught
"""
    m = pydantic_monty.Monty(code)

    def fail(*args: Any, **kwargs: Any) -> None:
        raise exception_class('test')

    # Child exception should be caught by parent handler (which comes first)
    result = m.run(external_functions={'fail': fail})
    assert result == 'parent'


@pytest.mark.parametrize(
    'exception_class,expected_result',
    [
        (ZeroDivisionError, 'ZeroDivisionError'),
        (OverflowError, 'OverflowError'),
        (NotImplementedError, 'NotImplementedError'),
        (RecursionError, 'RecursionError'),
        (KeyError, 'KeyError'),
        (IndexError, 'IndexError'),
    ],
)
def test_external_function_exception_caught_specifically(exception_class: type[BaseException], expected_result: str):
    """Test that child exceptions can be caught by their specific handler."""
    code = f"""
try:
    fail()
except {exception_class.__name__}:
    caught = '{expected_result}'
caught
"""
    m = pydantic_monty.Monty(code)

    def fail(*args: Any, **kwargs: Any) -> None:
        raise exception_class('test')

    result = m.run(external_functions={'fail': fail})
    assert result == expected_result


def test_external_function_exception_in_expression():
    """Test exception from external function in an expression context."""
    m = pydantic_monty.Monty('1 + fail() + 2')

    def fail(*args: Any, **kwargs: Any) -> int:
        raise RuntimeError('mid-expression error')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(external_functions={'fail': fail})
    inner = exc_info.value.exception()
    assert isinstance(inner, RuntimeError)
    assert inner.args[0] == snapshot('mid-expression error')


def test_external_function_exception_after_successful_call():
    """Test exception handling after a successful external call."""
    code = """
a = success()
b = fail()
a + b
"""
    m = pydantic_monty.Monty(code)

    def success(*args: Any, **kwargs: Any) -> int:
        return 10

    def fail(*args: Any, **kwargs: Any) -> int:
        raise ValueError('second call fails')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        m.run(external_functions={'success': success, 'fail': fail})
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert inner.args[0] == snapshot('second call fails')


def test_external_function_exception_with_finally():
    """Test that finally block runs when external function raises."""
    code = """
finally_ran = False
try:
    fail()
except ValueError:
    pass
finally:
    finally_ran = True
finally_ran
"""
    m = pydantic_monty.Monty(code)

    def fail(*args: Any, **kwargs: Any) -> None:
        raise ValueError('error')

    result = m.run(external_functions={'fail': fail})
    assert result == snapshot(True)
