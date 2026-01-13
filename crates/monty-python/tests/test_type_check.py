import pytest
from inline_snapshot import snapshot

import monty


def test_type_check_no_errors():
    """Type checking code with no errors returns None."""
    m = monty.Monty('x = 1')
    assert m.type_check() is None


def test_type_check_with_errors():
    """Type checking code with type errors raises MontyTypingError."""
    m = monty.Monty('"hello" + 1')
    with pytest.raises(monty.MontyTypingError) as exc_info:
        m.type_check()
    assert str(exc_info.value) == snapshot("""\
error[unsupported-operator]: Unsupported `+` operation
 --> main.py:1:1
  |
1 | "hello" + 1
  | -------^^^-
  | |         |
  | |         Has type `Literal[1]`
  | Has type `Literal["hello"]`
  |
info: rule `unsupported-operator` is enabled by default

""")


def test_type_check_function_return_type():
    """Type checking detects mismatched return types."""
    code = """
def foo() -> int:
    return "not an int"
"""
    m = monty.Monty(code)
    with pytest.raises(monty.MontyTypingError) as exc_info:
        m.type_check()
    assert str(exc_info.value) == snapshot("""\
error[invalid-return-type]: Return type does not match returned value
 --> main.py:2:14
  |
2 | def foo() -> int:
  |              --- Expected `int` because of return type
3 |     return "not an int"
  |            ^^^^^^^^^^^^ expected `int`, found `Literal["not an int"]`
  |
info: rule `invalid-return-type` is enabled by default

""")


def test_type_check_undefined_variable():
    """Type checking detects undefined variables."""
    m = monty.Monty('print(undefined_var)')
    with pytest.raises(monty.MontyTypingError) as exc_info:
        m.type_check()
    assert str(exc_info.value) == snapshot("""\
error[unresolved-reference]: Name `undefined_var` used when not defined
 --> main.py:1:7
  |
1 | print(undefined_var)
  |       ^^^^^^^^^^^^^
  |
info: rule `unresolved-reference` is enabled by default

""")


def test_type_check_valid_function():
    """Type checking valid function returns None."""
    code = """
def add(a: int, b: int) -> int:
    return a + b

add(1, 2)
"""
    m = monty.Monty(code)
    assert m.type_check() is None


def test_type_check_with_prefix_code():
    """Type checking with prefix code for input declarations."""
    m = monty.Monty('result = x + 1')
    # Without prefix, x is undefined
    with pytest.raises(monty.MontyTypingError):
        m.type_check()
    # With prefix declaring x as a variable, it should pass
    assert m.type_check(prefix_code='x = 0') is None


def test_type_check_display_invalid_format():
    """Invalid format string on display() raises ValueError."""
    m = monty.Monty('"hello" + 1')
    with pytest.raises(monty.MontyTypingError) as exc_info:
        m.type_check()
    with pytest.raises(ValueError) as val_exc:
        exc_info.value.display('invalid_format')  # pyright: ignore[reportArgumentType]
    assert str(val_exc.value) == snapshot('Unknown format: invalid_format')


def test_type_check_display_concise_format():
    """Type checking with concise format via display()."""
    m = monty.Monty('"hello" + 1')
    with pytest.raises(monty.MontyTypingError) as exc_info:
        m.type_check()
    assert exc_info.value.display('concise') == snapshot(
        'main.py:1:1: error[unsupported-operator] Operator `+` is not supported between objects of type `Literal["hello"]` and `Literal[1]`\n'
    )


# === MontyTypingError tests ===


def test_monty_typing_error_is_monty_error_subclass():
    """MontyTypingError is a subclass of MontyError."""
    m = monty.Monty('"hello" + 1')
    with pytest.raises(monty.MontyTypingError) as exc_info:
        m.type_check()
    error = exc_info.value
    assert isinstance(error, monty.MontyError)
    assert isinstance(error, Exception)


def test_monty_typing_error_repr():
    """MontyTypingError has proper repr with truncation."""
    m = monty.Monty('"hello" + 1')
    with pytest.raises(monty.MontyTypingError) as exc_info:
        m.type_check()
    # repr truncates at 50 chars
    assert repr(exc_info.value) == snapshot("""\
MontyTypingError(error[unsupported-operator]: Unsupported `+` operation
 --> main.py:1:1
  |
1 | "hello" + 1
  | -------^^^-
  | |         |
  | |         Has type `Literal[1]`
  | Has type `Literal["hello"]`
  |
info: rule `unsupported-operator` is enabled by default

)\
""")


def test_monty_typing_error_caught_as_monty_error():
    """MontyTypingError can be caught as MontyError."""
    m = monty.Monty('"hello" + 1')
    with pytest.raises(monty.MontyError):
        m.type_check()


def test_monty_typing_error_display_default():
    """MontyTypingError display() defaults to full format."""
    m = monty.Monty('"hello" + 1')
    with pytest.raises(monty.MontyTypingError) as exc_info:
        m.type_check()
    # Default display should match str()
    assert exc_info.value.display() == str(exc_info.value)
