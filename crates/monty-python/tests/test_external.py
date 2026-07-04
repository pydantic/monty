import datetime
import io
import pathlib
import re
from typing import Any

import pytest
from conftest import RunMonty
from inline_snapshot import snapshot

import pydantic_monty
from pydantic_monty import MontySession


def test_external_function_no_args(monty_run: RunMonty):
    def noop(*args: Any, **kwargs: Any) -> str:
        assert args == snapshot(())
        assert kwargs == snapshot({})
        return 'called'

    assert monty_run('noop()', external_lookup={'noop': noop}) == snapshot('called')


def test_external_function_positional_args(monty_run: RunMonty):
    def func(*args: Any, **kwargs: Any) -> str:
        assert args == snapshot((1, 2, 3))
        assert kwargs == snapshot({})
        return 'ok'

    assert monty_run('func(1, 2, 3)', external_lookup={'func': func}) == snapshot('ok')


def test_external_function_kwargs_only(monty_run: RunMonty):
    def func(*args: Any, **kwargs: Any) -> str:
        assert args == snapshot(())
        assert kwargs == snapshot({'a': 1, 'b': 'two'})
        return 'ok'

    assert monty_run('func(a=1, b="two")', external_lookup={'func': func}) == snapshot('ok')


def test_external_function_mixed_args_kwargs(monty_run: RunMonty):
    def func(*args: Any, **kwargs: Any) -> str:
        assert args == snapshot((1, 2))
        assert kwargs == snapshot({'x': 'hello', 'y': True})
        return 'ok'

    assert monty_run('func(1, 2, x="hello", y=True)', external_lookup={'func': func}) == snapshot('ok')


def test_external_function_complex_types(monty_run: RunMonty):
    def func(*args: Any, **kwargs: Any) -> str:
        assert args == snapshot(([1, 2], {'key': 'value'}))
        assert kwargs == snapshot({})
        return 'ok'

    assert monty_run('func([1, 2], {"key": "value"})', external_lookup={'func': func}) == snapshot('ok')


def test_external_function_type_objects(monty_run: RunMonty):
    """A type object passed as an external-call argument reconstructs as the matching
    host type; modeled stdlib types resolve from their real module (`Path` → `PurePosixPath`)."""
    code = """
import datetime
import re
from pathlib import Path
func(
    type(1),
    type('x'),
    type(int),
    type(Path('/x')),
    Path,
    datetime.datetime,
    datetime.date,
    datetime.timedelta,
    type(re.compile('a')),
    type(re.match('a', 'a')),
)
"""

    def func(*args: Any, **kwargs: Any) -> str:
        assert args == (
            int,
            str,
            type,
            pathlib.PurePosixPath,
            pathlib.PurePosixPath,
            datetime.datetime,
            datetime.date,
            datetime.timedelta,
            re.Pattern,
            re.Match,
        )
        assert kwargs == {}
        return 'ok'

    assert monty_run(code, external_lookup={'func': func}) == snapshot('ok')


def test_external_function_returns_instances(monty_run: RunMonty):
    """A host callback returning an instance of a modeled stdlib type marshals back
    into the sandbox (datetime family and `pathlib` paths round-trip)."""
    code = """
results = []
for v in (get_dt(), get_dt_tz(), get_date(), get_delta(), get_tz(), get_path()):
    results.append((type(v).__name__, repr(v)))
results
"""
    fns: dict[str, Any] = {
        'get_dt': lambda: datetime.datetime(2021, 1, 2, 3, 4, 5),
        'get_dt_tz': lambda: datetime.datetime(2021, 1, 2, 3, 4, 5, tzinfo=datetime.timezone.utc),
        'get_date': lambda: datetime.date(2021, 1, 2),
        'get_delta': lambda: datetime.timedelta(days=1, seconds=2),
        'get_tz': lambda: datetime.timezone(datetime.timedelta(hours=5)),
        'get_path': lambda: pathlib.PurePosixPath('/a/b'),
    }
    assert monty_run(code, external_lookup=fns) == snapshot(
        [
            ('datetime.datetime', 'datetime.datetime(2021, 1, 2, 3, 4, 5)'),
            ('datetime.datetime', 'datetime.datetime(2021, 1, 2, 3, 4, 5, tzinfo=datetime.timezone.utc)'),
            ('date', 'datetime.date(2021, 1, 2)'),
            ('timedelta', 'datetime.timedelta(days=1, seconds=2)'),
            ('timezone', 'datetime.timezone(datetime.timedelta(seconds=18000))'),
            ('PosixPath', "PosixPath('/a/b')"),
        ]
    )


@pytest.mark.parametrize(
    'factory, message',
    [
        (lambda: re.compile('a'), snapshot('Cannot convert re.Pattern to Monty value')),
        (lambda: re.match('a', 'a'), snapshot('Cannot convert re.Match to Monty value')),
        (lambda: io.StringIO('x'), snapshot('Cannot convert _io.StringIO to Monty value')),
    ],
)
def test_external_function_returns_unconvertible_instance(monty_run: RunMonty, factory: Any, message: str):
    """Instances of types Monty does not model (`re.Pattern`, `re.Match`, host
    file objects, …) cannot cross back into the sandbox; the return-value
    conversion fails as a `TypeError` that is catchable inside Monty."""
    code = """
try:
    get_thing()
    result = 'no error'
except TypeError as e:
    result = str(e)
result
"""
    assert monty_run(code, external_lookup={'get_thing': factory}) == message


def test_external_function_returns_none(monty_run: RunMonty):
    def do_nothing(*args: Any, **kwargs: Any) -> None:
        assert args == snapshot(())
        assert kwargs == snapshot({})

    assert monty_run('do_nothing()', external_lookup={'do_nothing': do_nothing}) is None


def test_external_function_returns_complex_type(monty_run: RunMonty):
    def get_data(*args: Any, **kwargs: Any) -> dict[str, Any]:
        return {'a': [1, 2, 3], 'b': {'nested': True}}

    result = monty_run('get_data()', external_lookup={'get_data': get_data})
    assert result == snapshot({'a': [1, 2, 3], 'b': {'nested': True}})


def test_multiple_external_lookup(monty_run: RunMonty):
    def add(*args: Any, **kwargs: Any) -> int:
        assert args == snapshot((1, 2))
        assert kwargs == snapshot({})
        return args[0] + args[1]

    def mul(*args: Any, **kwargs: Any) -> int:
        assert args == snapshot((3, 4))
        assert kwargs == snapshot({})
        return args[0] * args[1]

    result = monty_run('add(1, 2) + mul(3, 4)', external_lookup={'add': add, 'mul': mul})
    assert result == snapshot(15)  # 3 + 12


def test_external_function_called_multiple_times(monty_run: RunMonty):
    call_count = 0

    def counter(*args: Any, **kwargs: Any) -> int:
        nonlocal call_count
        assert args == snapshot(())
        assert kwargs == snapshot({})
        call_count += 1
        return call_count

    result = monty_run('counter() + counter() + counter()', external_lookup={'counter': counter})
    assert result == snapshot(6)  # 1 + 2 + 3
    assert call_count == snapshot(3)


def test_external_function_with_input(monty_run: RunMonty):
    def process(*args: Any, **kwargs: Any) -> int:
        assert args == snapshot((5,))
        assert kwargs == snapshot({})
        return args[0] * 10

    assert monty_run('process(x)', inputs={'x': 5}, external_lookup={'process': process}) == snapshot(50)


def test_external_function_not_provided_raises_name_error(monty_run: RunMonty):
    """Calling an unknown function without external_lookup raises NameError."""
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('missing()')
    inner = exc_info.value.exception()
    assert type(inner) is NameError
    assert str(inner) == snapshot("name 'missing' is not defined")


def test_undeclared_function_raises_name_error(monty_run: RunMonty):
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('unknown_func()')
    inner = exc_info.value.exception()
    assert type(inner) is NameError
    assert str(inner) == snapshot("name 'unknown_func' is not defined")


def test_external_function_raises_exception(monty_run: RunMonty):
    """Test that exceptions from external functions propagate to the caller."""

    def fail(*args: Any, **kwargs: Any) -> None:
        raise ValueError('intentional error')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('fail()', external_lookup={'fail': fail})
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert inner.args[0] == snapshot('intentional error')


def test_external_function_wrong_name_raises(monty_run: RunMonty):
    """Test that calling a function not in external_lookup raises NameError."""

    def bar(*args: Any, **kwargs: Any) -> int:
        return 1

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('foo()', external_lookup={'bar': bar})
    inner = exc_info.value.exception()
    assert type(inner) is NameError
    assert str(inner) == snapshot("name 'foo' is not defined")


def test_external_function_exception_caught_by_try_except(monty_run: RunMonty):
    """Test that exceptions from external functions can be caught by try/except."""
    code = """
try:
    fail()
except ValueError:
    caught = True
caught
"""

    def fail(*args: Any, **kwargs: Any) -> None:
        raise ValueError('caught error')

    result = monty_run(code, external_lookup={'fail': fail})
    assert result == snapshot(True)


def test_external_function_exception_type_preserved(monty_run: RunMonty):
    """Test that various exception types are correctly preserved."""

    def fail_type_error(*args: Any, **kwargs: Any) -> None:
        raise TypeError('type error message')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('fail()', external_lookup={'fail': fail_type_error})
    inner = exc_info.value.exception()
    assert isinstance(inner, TypeError)
    assert inner.args[0] == snapshot('type error message')


def test_external_function_unsupported_operation_preserves_type(monty_run: RunMonty):
    """`io.UnsupportedOperation` survives the host→Monty→host round-trip.

    Regression: the exception inherits from both `OSError` and `ValueError`,
    so a naive `py_err_to_exc_type` would hit the `ValueError` branch first
    and downgrade it to plain `ExcType::ValueError`, losing the class identity.
    """

    def fail(*args: Any, **kwargs: Any) -> None:
        raise io.UnsupportedOperation('not readable')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('fail()', external_lookup={'fail': fail})
    inner = exc_info.value.exception()
    assert type(inner) is io.UnsupportedOperation
    assert isinstance(inner, io.UnsupportedOperation)
    # And the dual-parent catch behavior is preserved in Monty code too:
    assert isinstance(inner, OSError)
    assert isinstance(inner, ValueError)
    assert inner.args[0] == snapshot('not readable')


@pytest.mark.parametrize('parent', ['OSError', 'ValueError'])
def test_external_unsupported_operation_caught_by_either_parent(monty_run: RunMonty, parent: str):
    """`except OSError:` and `except ValueError:` both catch a host-raised
    `io.UnsupportedOperation`, matching CPython's dual inheritance."""
    code = f"""
try:
    fail()
except {parent}:
    caught = '{parent}'
caught
"""

    def fail(*args: Any, **kwargs: Any) -> None:
        raise io.UnsupportedOperation('boom')

    assert monty_run(code, external_lookup={'fail': fail}) == parent


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
def test_external_function_exception_hierarchy(
    monty_run: RunMonty, exception_class: type[BaseException], exception_name: str
):
    """Test that exception types in hierarchies are correctly preserved."""

    def fail(*args: Any, **kwargs: Any) -> None:
        raise exception_class('test message')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('fail()', external_lookup={'fail': fail})
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
    monty_run: RunMonty,
    exception_class: type[BaseException],
    parent_class: type[BaseException],
    expected_result: str,
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

    def fail(*args: Any, **kwargs: Any) -> None:
        raise exception_class('test')

    # Child exception should be caught by parent handler (which comes first)
    result = monty_run(code, external_lookup={'fail': fail})
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
def test_external_function_exception_caught_specifically(
    monty_run: RunMonty, exception_class: type[BaseException], expected_result: str
):
    """Test that child exceptions can be caught by their specific handler."""
    code = f"""
try:
    fail()
except {exception_class.__name__}:
    caught = '{expected_result}'
caught
"""

    def fail(*args: Any, **kwargs: Any) -> None:
        raise exception_class('test')

    result = monty_run(code, external_lookup={'fail': fail})
    assert result == expected_result


def test_external_function_exception_in_expression(monty_run: RunMonty):
    """Test exception from external function in an expression context."""

    def fail(*args: Any, **kwargs: Any) -> int:
        raise RuntimeError('mid-expression error')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('1 + fail() + 2', external_lookup={'fail': fail})
    inner = exc_info.value.exception()
    assert isinstance(inner, RuntimeError)
    assert inner.args[0] == snapshot('mid-expression error')


def test_external_function_exception_after_successful_call(monty_run: RunMonty):
    """Test exception handling after a successful external call."""
    code = """
a = success()
b = fail()
a + b
"""

    def success(*args: Any, **kwargs: Any) -> int:
        return 10

    def fail(*args: Any, **kwargs: Any) -> int:
        raise ValueError('second call fails')

    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run(code, external_lookup={'success': success, 'fail': fail})
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert inner.args[0] == snapshot('second call fails')


def test_external_function_exception_with_finally(monty_run: RunMonty):
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

    def fail(*args: Any, **kwargs: Any) -> None:
        raise ValueError('error')

    result = monty_run(code, external_lookup={'fail': fail})
    assert result == snapshot(True)


def test_external_function_return_lone_surrogate_catchable_inside_monty(monty_run: RunMonty):
    """A callback returning a string with a lone surrogate surfaces inside
    Monty as a `ValueError` that can be caught, not as a raw PyErr escaping
    to the caller."""
    code = """
try:
    get_str()
    result = 'no error'
except ValueError:
    result = 'caught'
result
"""
    assert monty_run(code, external_lookup={'get_str': lambda: '\ud83d'}) == snapshot('caught')


def test_external_function_return_unconvertible_catchable_inside_monty(monty_run: RunMonty):
    """A callback returning an unconvertible object surfaces inside Monty as a
    `TypeError` that can be caught."""
    code = """
try:
    get_thing()
    result = 'no error'
except TypeError:
    result = 'caught'
result
"""
    assert monty_run(code, external_lookup={'get_thing': lambda: object()}) == snapshot('caught')


# =============================================================================
# external_lookup value resolution (non-callable entries)
# =============================================================================


def test_external_lookup_value(monty_run: RunMonty):
    """A non-callable entry resolves the bare name to that converted value."""
    assert monty_run('x + 1', external_lookup={'x': 41}) == snapshot(42)


def test_external_lookup_value_none(monty_run: RunMonty):
    """A `None` entry is a present name resolving to `None`, not a `NameError`
    (pins consistency with the JS bindings' `null`/`undefined` entries)."""
    assert monty_run('x is None', external_lookup={'x': None}) == snapshot(True)


def test_external_lookup_value_container(monty_run: RunMonty):
    """Container values convert and round-trip through a name lookup."""
    assert monty_run('data["a"] + data["b"]', external_lookup={'data': {'a': 1, 'b': 2}}) == snapshot(3)


def test_external_lookup_mixed_function_and_value(monty_run: RunMonty):
    """One dict can carry both a callable (function proxy) and a plain value."""

    def double(x: int) -> int:
        return x * 2

    assert monty_run('double(n)', external_lookup={'double': double, 'n': 21}) == snapshot(42)


def test_external_lookup_value_repeated_reference(monty_run: RunMonty):
    """Referencing the same lazily-resolved name twice in one feed works. The
    worker caches the resolved value, but dict reads are not observable
    host-side (`get_item` bypasses subclass hooks), so this only pins the
    result; the JS test observes the single read via a getter."""
    assert monty_run('x + x', external_lookup={'x': 21}) == snapshot(42)


def test_external_lookup_absent_name_raises(monty_run: RunMonty):
    """A name absent from external_lookup raises NameError (not the value path)."""
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        monty_run('missing', external_lookup={'present': 1})
    inner = exc_info.value.exception()
    assert type(inner) is NameError
    assert str(inner) == snapshot("name 'missing' is not defined")


def test_external_lookup_value_unconvertible_surfaces_error(monty_run: RunMonty):
    """A non-callable value of an unsupported type surfaces a conversion error,
    not a misleading NameError."""
    with pytest.raises(TypeError) as exc_info:
        monty_run('x', external_lookup={'x': object()})
    assert str(exc_info.value) == snapshot('Cannot convert builtins.object to Monty value')


def test_external_lookup_type_object_round_trips(monty_run: RunMonty):
    """A modeled type object resolves a bare name to the Monty type (so
    `isinstance` works), rather than degrading to a host-function proxy just
    because a type is callable."""
    assert monty_run('isinstance(5, IntType)', external_lookup={'IntType': int}) == snapshot(True)
    assert monty_run('isinstance(5, StrType)', external_lookup={'StrType': str}) == snapshot(False)


def test_external_lookup_stale_proxy_not_callable(session: MontySession):
    """A function proxy cached in one feed dispatches by name against the
    *current* dict on each call: with the entry replaced by a plain value,
    calling it raises the TypeError CPython would for calling that value (the
    JS bindings synthesize the same error)."""

    def double(x: int) -> int:
        return x * 2

    session.feed_run('f = double', external_lookup={'double': double})
    with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
        session.feed_run('f(2)', external_lookup={'double': 5})
    inner = exc_info.value.exception()
    assert type(inner) is TypeError
    assert str(inner) == snapshot("'int' object is not callable")


def test_external_lookup_name_conversion_error_discards_session(session: MontySession):
    """A conversion failure while resolving a bare name discards the suspended
    worker rather than wedging it: the feed raises, and a follow-up feed on the
    same session fails fast instead of hanging on a dangling name-lookup
    suspension the aborted feed never answered."""
    with pytest.raises(TypeError) as exc_info:
        session.feed_run('x', external_lookup={'x': object()})
    assert str(exc_info.value) == snapshot('Cannot convert builtins.object to Monty value')
    # the worker was discarded, so the session can no longer be fed
    with pytest.raises(RuntimeError) as exc_info2:
        session.feed_run('1 + 1')
    assert str(exc_info2.value) == snapshot('this checkout has already been finished')
