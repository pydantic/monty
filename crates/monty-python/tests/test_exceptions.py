import pytest
from inline_snapshot import snapshot

import monty

# === MontyRuntimeError tests ===


def test_zero_division_error():
    m = monty.Monty('1 / 0')
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    # Check that it's also a MontyError
    assert isinstance(exc_info.value, monty.MontyError)
    # Check the inner exception
    inner = exc_info.value.exception()
    assert isinstance(inner, ZeroDivisionError)


def test_value_error():
    m = monty.Monty("raise ValueError('bad value')")
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert str(inner) == snapshot('bad value')


def test_type_error():
    m = monty.Monty("'string' + 1")
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    inner = exc_info.value.exception()
    assert isinstance(inner, TypeError)


def test_index_error():
    m = monty.Monty('[1, 2, 3][10]')
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    inner = exc_info.value.exception()
    assert isinstance(inner, IndexError)


def test_key_error():
    m = monty.Monty("{'a': 1}['b']")
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    inner = exc_info.value.exception()
    assert isinstance(inner, KeyError)


def test_attribute_error():
    m = monty.Monty("raise AttributeError('no such attr')")
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    inner = exc_info.value.exception()
    assert isinstance(inner, AttributeError)
    assert str(inner) == snapshot('no such attr')


def test_name_error():
    m = monty.Monty('undefined_variable')
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    inner = exc_info.value.exception()
    assert isinstance(inner, NameError)


def test_assertion_error():
    m = monty.Monty('assert False')
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    inner = exc_info.value.exception()
    assert isinstance(inner, AssertionError)


def test_assertion_error_with_message():
    m = monty.Monty("assert False, 'custom message'")
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    inner = exc_info.value.exception()
    assert isinstance(inner, AssertionError)
    assert str(inner) == snapshot('custom message')


def test_runtime_error():
    m = monty.Monty("raise RuntimeError('runtime error')")
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    inner = exc_info.value.exception()
    assert isinstance(inner, RuntimeError)
    assert str(inner) == snapshot('runtime error')


def test_not_implemented_error():
    m = monty.Monty("raise NotImplementedError('not implemented')")
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    inner = exc_info.value.exception()
    assert isinstance(inner, NotImplementedError)
    assert str(inner) == snapshot('not implemented')


# === MontySyntaxError tests ===


def test_syntax_error_on_init():
    with pytest.raises(monty.MontySyntaxError) as exc_info:
        monty.Monty('def')
    # Check that it's also a MontyError
    assert isinstance(exc_info.value, monty.MontyError)
    # Check the inner exception
    inner = exc_info.value.exception()
    assert isinstance(inner, SyntaxError)


def test_syntax_error_unclosed_paren():
    with pytest.raises(monty.MontySyntaxError) as exc_info:
        monty.Monty('print(1')
    inner = exc_info.value.exception()
    assert isinstance(inner, SyntaxError)


def test_syntax_error_invalid_syntax():
    with pytest.raises(monty.MontySyntaxError) as exc_info:
        monty.Monty('x = = 1')
    inner = exc_info.value.exception()
    assert isinstance(inner, SyntaxError)


# === Catching with base class ===


def test_catch_with_base_class():
    m = monty.Monty('1 / 0')
    with pytest.raises(monty.MontyError):
        m.run()


def test_catch_syntax_error_with_base_class():
    with pytest.raises(monty.MontyError):
        monty.Monty('def')


# === Exception handling within Monty ===


def test_raise_caught_exception():
    code = """
try:
    1 / 0
except ZeroDivisionError as e:
    result = 'caught'
result
"""
    m = monty.Monty(code)
    assert m.run() == snapshot('caught')


def test_exception_in_function():
    code = """
def fail():
    raise ValueError('from function')

fail()
"""
    m = monty.Monty(code)
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert str(inner) == snapshot('from function')


# === Display and str methods ===


def test_display_traceback():
    m = monty.Monty('1 / 0')
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    display = exc_info.value.display()
    assert 'Traceback (most recent call last):' in display
    assert 'ZeroDivisionError' in display


def test_display_type_msg():
    m = monty.Monty("raise ValueError('test message')")
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    display = exc_info.value.display('type-msg')
    assert display == snapshot('ValueError: test message')


def test_display_msg():
    m = monty.Monty("raise ValueError('test message')")
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    display = exc_info.value.display('msg')
    assert display == snapshot('test message')


def test_str_returns_msg():
    m = monty.Monty("raise ValueError('test message')")
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    assert str(exc_info.value) == snapshot('test message')


def test_syntax_error_display():
    with pytest.raises(monty.MontySyntaxError) as exc_info:
        monty.Monty('def')
    display = exc_info.value.display()
    assert 'SyntaxError:' in display


def test_syntax_error_str():
    with pytest.raises(monty.MontySyntaxError) as exc_info:
        monty.Monty('def')
    # str() returns just the message
    assert 'SyntaxError' not in str(exc_info.value)


# === Traceback tests ===


def test_traceback_frames():
    code = """\
def inner():
    raise ValueError('error')

def outer():
    inner()

outer()
"""
    m = monty.Monty(code)
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    frames = exc_info.value.traceback()
    assert isinstance(frames, list)
    assert len(frames) >= 2  # At least module level, outer(), and inner()

    assert exc_info.value.display() == snapshot("""\
Traceback (most recent call last):
  File "main.py", line 7, in <module>
    outer()
    ~~~~~~~
  File "main.py", line 5, in outer
    inner()
    ~~~~~~~
  File "main.py", line 2, in inner
    raise ValueError('error')
ValueError: error\
""")

    assert [f.dict() for f in frames] == snapshot(
        [
            {
                'filename': 'main.py',
                'line': 7,
                'column': 1,
                'end_line': 7,
                'end_column': 8,
                'function_name': '<module>',
                'source_line': 'outer()',
            },
            {
                'filename': 'main.py',
                'line': 5,
                'column': 5,
                'end_line': 5,
                'end_column': 12,
                'function_name': 'outer',
                'source_line': '    inner()',
            },
            {
                'filename': 'main.py',
                'line': 2,
                'column': 11,
                'end_line': 2,
                'end_column': 30,
                'function_name': 'inner',
                'source_line': "    raise ValueError('error')",
            },
        ]
    )


def test_frame_properties():
    code = """
def foo():
    raise ValueError('test')

foo()
"""
    m = monty.Monty(code)
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    frames = exc_info.value.traceback()

    assert [f.dict() for f in frames] == snapshot(
        [
            {
                'filename': 'main.py',
                'line': 5,
                'column': 1,
                'end_line': 5,
                'end_column': 6,
                'function_name': '<module>',
                'source_line': 'foo()',
            },
            {
                'filename': 'main.py',
                'line': 3,
                'column': 11,
                'end_line': 3,
                'end_column': 29,
                'function_name': 'foo',
                'source_line': "    raise ValueError('test')",
            },
        ]
    )


# === Repr tests ===


def test_runtime_error_repr():
    m = monty.Monty("raise ValueError('test')")
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    assert repr(exc_info.value) == snapshot('MontyRuntimeError(ValueError: test)')


def test_syntax_error_repr():
    with pytest.raises(monty.MontySyntaxError) as exc_info:
        monty.Monty('def')
    assert repr(exc_info.value) == snapshot('MontySyntaxError(Expected an identifier at byte range 3..3)')


def test_frame_repr():
    code = """
def foo():
    raise ValueError('test')

foo()
"""
    m = monty.Monty(code)
    with pytest.raises(monty.MontyRuntimeError) as exc_info:
        m.run()
    frames = exc_info.value.traceback()
    frame = frames[0]
    assert repr(frame) == snapshot("Frame(filename='main.py', line=5, column=1, function_name='<module>')")
