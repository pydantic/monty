"""Error-mapping tests: MontyRuntimeError / MontySyntaxError shape, display and tracebacks."""

from __future__ import annotations

import pytest
from conftest import RunMonty
from inline_snapshot import snapshot

from pydantic_monty import Monty, MontyError, MontyRuntimeError, MontySyntaxError

# === MontyRuntimeError tests ===


def test_zero_division_error(monty_run: RunMonty):
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('1 / 0')
    # Check that it's also a MontyError
    assert isinstance(exc_info.value, MontyError)
    # Check the inner exception
    inner = exc_info.value.exception()
    assert isinstance(inner, ZeroDivisionError)


def test_value_error(monty_run: RunMonty):
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("raise ValueError('bad value')")
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert str(inner) == snapshot('bad value')


def test_unicode_encode_error(monty_run: RunMonty):
    # `str.encode('ascii')` on a non-ascii string raises `UnicodeEncodeError`
    # inside the sandbox; the structured constructor fields travel with the
    # exception so `.exception()` rebuilds the real `UnicodeEncodeError`.
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("'café'.encode('ascii')")
    inner = exc_info.value.exception()
    assert isinstance(inner, UnicodeEncodeError)
    assert inner.encoding == snapshot('ascii')
    assert inner.object == snapshot('café')
    assert inner.start == snapshot(3)
    assert inner.end == snapshot(4)
    assert inner.reason == snapshot('ordinal not in range(128)')
    assert str(inner) == snapshot(
        "'ascii' codec can't encode character '\\xe9' in position 3: ordinal not in range(128)"
    )


def test_unicode_decode_error(monty_run: RunMonty):
    # `bytes.decode('ascii')` on non-ascii bytes raises `UnicodeDecodeError`
    # inside the sandbox; as in `test_unicode_encode_error`, `.exception()`
    # rebuilds the real `UnicodeDecodeError` from the structured fields.
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("b'\\xe9'.decode('ascii')")
    inner = exc_info.value.exception()
    assert isinstance(inner, UnicodeDecodeError)
    assert inner.encoding == snapshot('ascii')
    assert inner.object == snapshot(b'\xe9')
    assert inner.start == snapshot(0)
    assert inner.end == snapshot(1)
    assert inner.reason == snapshot('ordinal not in range(128)')
    assert str(inner) == snapshot("'ascii' codec can't decode byte 0xe9 in position 0: ordinal not in range(128)")


def test_unicode_error_message_only_fallback(monty_run: RunMonty):
    # A `UnicodeDecodeError` raised manually inside the sandbox has no
    # structured fields (Monty exception constructors are message-only), so
    # `.exception()` falls back to a `ValueError` carrying the message.
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("raise UnicodeDecodeError('nope')")
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert not isinstance(inner, UnicodeDecodeError)
    assert str(inner) == snapshot('nope')


def test_type_error(monty_run: RunMonty):
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("'string' + 1")
    assert isinstance(exc_info.value.exception(), TypeError)


def test_index_error(monty_run: RunMonty):
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('[1, 2, 3][10]')
    assert isinstance(exc_info.value.exception(), IndexError)


def test_key_error(monty_run: RunMonty):
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("{'a': 1}['b']")
    assert isinstance(exc_info.value.exception(), KeyError)


def test_attribute_error(monty_run: RunMonty):
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("raise AttributeError('no such attr')")
    inner = exc_info.value.exception()
    assert isinstance(inner, AttributeError)
    assert str(inner) == snapshot('no such attr')


def test_name_error(monty_run: RunMonty):
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('undefined_variable')
    assert isinstance(exc_info.value.exception(), NameError)


def test_assertion_error(monty_run: RunMonty):
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('assert False')
    assert isinstance(exc_info.value.exception(), AssertionError)


def test_assertion_error_with_message(monty_run: RunMonty):
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("assert False, 'custom message'")
    inner = exc_info.value.exception()
    assert isinstance(inner, AssertionError)
    assert str(inner) == snapshot('custom message')


def test_runtime_error(monty_run: RunMonty):
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("raise RuntimeError('runtime error')")
    inner = exc_info.value.exception()
    assert isinstance(inner, RuntimeError)
    assert str(inner) == snapshot('runtime error')


def test_not_implemented_error(monty_run: RunMonty):
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("raise NotImplementedError('not implemented')")
    inner = exc_info.value.exception()
    assert isinstance(inner, NotImplementedError)
    assert str(inner) == snapshot('not implemented')


# === MontySyntaxError tests ===
# Syntax errors surface at feed_run time, not at construction.


def test_syntax_error_on_feed(monty_run: RunMonty):
    with pytest.raises(MontySyntaxError) as exc_info:
        monty_run('def')
    # Check that it's also a MontyError
    assert isinstance(exc_info.value, MontyError)
    # Check the inner exception
    inner = exc_info.value.exception()
    assert isinstance(inner, SyntaxError)


def test_syntax_error_unclosed_paren(monty_run: RunMonty):
    with pytest.raises(MontySyntaxError) as exc_info:
        monty_run('print(1')
    assert isinstance(exc_info.value.exception(), SyntaxError)


def test_syntax_error_invalid_syntax(monty_run: RunMonty):
    with pytest.raises(MontySyntaxError) as exc_info:
        monty_run('x = = 1')
    assert isinstance(exc_info.value.exception(), SyntaxError)


def test_syntax_error_lone_surrogate(monty_run: RunMonty):
    # Lone surrogates cannot be encoded as UTF-8, so they are not valid Python
    # source. feed_run reports this as MontySyntaxError rather than letting
    # PyO3's raw UnicodeEncodeError bubble out.
    with pytest.raises(MontySyntaxError) as exc_info:
        monty_run('\ud83d')
    assert str(exc_info.value) == snapshot('source code is not valid UTF-8 (contains lone surrogates)')
    inner = exc_info.value.exception()
    assert isinstance(inner, SyntaxError)


def test_runtime_error_input_value_lone_surrogate(monty_run: RunMonty):
    # An input string containing a lone surrogate fails UTF-8 conversion during
    # `py_to_monty`, raising a real `UnicodeEncodeError` (a `ValueError`
    # subclass). `.exception()` falls back to a plain `ValueError` carrying
    # the same message rather than `UnicodeEncodeError`, since Monty only
    # stores the formatted message and CPython's real `UnicodeEncodeError`
    # constructor requires 5 positional args (`encoding, object, start, end,
    # reason`) that a single string can't satisfy.
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('x', inputs={'x': '\ud83d'})
    assert str(exc_info.value) == snapshot(
        "UnicodeEncodeError: 'utf-8' codec can't encode character '\\ud83d' in position 0: surrogates not allowed"
    )
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)


def test_runtime_error_input_key_lone_surrogate(monty_run: RunMonty):
    # An input *key* containing a lone surrogate also goes through UTF-8
    # conversion; wrap it the same way.
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('x', inputs={'\ud83d': 1})
    assert isinstance(exc_info.value.exception(), ValueError)


def test_syntax_error_stubs_lone_surrogate(pool: Monty):
    # Stubs are parsed as Python source, so invalid UTF-8 is not valid source
    # text. We surface this as `MontySyntaxError` rather than letting PyO3's
    # `UnicodeEncodeError` bubble up.
    with pytest.raises(MontySyntaxError) as exc_info:
        with pool.checkout(type_check=True, type_check_stubs='\ud83d') as session:
            session.feed_run('1')
    assert str(exc_info.value) == snapshot('type_check_stubs is not valid UTF-8')


# === Catching with base class ===


def test_catch_with_base_class(monty_run: RunMonty):
    with pytest.raises(MontyError):
        monty_run('1 / 0')


def test_catch_syntax_error_with_base_class(monty_run: RunMonty):
    with pytest.raises(MontyError):
        monty_run('def')


# === Exception handling within Monty ===


def test_raise_caught_exception(monty_run: RunMonty):
    code = """
try:
    1 / 0
except ZeroDivisionError as e:
    result = 'caught'
result
"""
    assert monty_run(code) == snapshot('caught')


def test_exception_in_function(monty_run: RunMonty):
    code = """
def fail():
    raise ValueError('from function')

fail()
"""
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run(code)
    inner = exc_info.value.exception()
    assert isinstance(inner, ValueError)
    assert str(inner) == snapshot('from function')


# === Display and str methods ===


def test_display_traceback(monty_run: RunMonty):
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('1 / 0')
    display = exc_info.value.display()
    assert 'Traceback (most recent call last):' in display
    assert 'ZeroDivisionError' in display


def test_display_type_msg(monty_run: RunMonty):
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("raise ValueError('test message')")
    assert exc_info.value.display('type-msg') == snapshot('ValueError: test message')


def test_runtime_display(monty_run: RunMonty):
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("raise ValueError('test message')")
    assert exc_info.value.display('msg') == snapshot('test message')
    assert exc_info.value.display('type-msg') == snapshot('ValueError: test message')
    # traceback filenames are `<python-input-N>` style in the session/REPL model
    assert exc_info.value.display() == snapshot("""\
Traceback (most recent call last):
  File "<python-input-0>", line 1, in <module>
    raise ValueError('test message')
ValueError: test message\
""")


def test_str_returns_msg(monty_run: RunMonty):
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("raise ValueError('test message')")
    assert str(exc_info.value) == snapshot('ValueError: test message')


def test_syntax_error_display(monty_run: RunMonty):
    with pytest.raises(MontySyntaxError) as exc_info:
        monty_run('def')
    assert exc_info.value.display() == snapshot("""\
Traceback (most recent call last):
  File "<python-input-0>", line 1
    def
       ~
SyntaxError: Expected an identifier\
""")
    assert exc_info.value.display('type-msg') == snapshot('SyntaxError: Expected an identifier')
    assert exc_info.value.display('msg') == snapshot('Expected an identifier')


def test_syntax_error_str(monty_run: RunMonty):
    with pytest.raises(MontySyntaxError) as exc_info:
        monty_run('def')
    # str() returns just the message
    assert 'SyntaxError' not in str(exc_info.value)


# === Traceback tests ===


def test_traceback_frames(monty_run: RunMonty):
    code = """\
def inner():
    raise ValueError('error')

def outer():
    inner()

outer()
"""
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run(code)
    frames = exc_info.value.traceback()
    assert isinstance(frames, list)
    assert len(frames) >= 2  # At least module level, outer(), and inner()

    assert exc_info.value.display() == snapshot("""\
Traceback (most recent call last):
  File "<python-input-0>", line 7, in <module>
    outer()
    ~~~~~~~
  File "<python-input-0>", line 5, in outer
    inner()
    ~~~~~~~
  File "<python-input-0>", line 2, in inner
    raise ValueError('error')
ValueError: error\
""")

    assert [f.dict() for f in frames] == snapshot(
        [
            {
                'filename': '<python-input-0>',
                'line': 7,
                'column': 1,
                'end_line': 7,
                'end_column': 8,
                'function_name': '<module>',
                'source_line': 'outer()',
            },
            {
                'filename': '<python-input-0>',
                'line': 5,
                'column': 5,
                'end_line': 5,
                'end_column': 12,
                'function_name': 'outer',
                'source_line': '    inner()',
            },
            {
                'filename': '<python-input-0>',
                'line': 2,
                'column': 11,
                'end_line': 2,
                'end_column': 30,
                'function_name': 'inner',
                'source_line': "    raise ValueError('error')",
            },
        ]
    )


def test_frame_properties(monty_run: RunMonty):
    code = """
def foo():
    raise ValueError('test')

foo()
"""
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run(code)
    frames = exc_info.value.traceback()

    assert [f.dict() for f in frames] == snapshot(
        [
            {
                'filename': '<python-input-0>',
                'line': 5,
                'column': 1,
                'end_line': 5,
                'end_column': 6,
                'function_name': '<module>',
                'source_line': 'foo()',
            },
            {
                'filename': '<python-input-0>',
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


def test_runtime_error_repr(monty_run: RunMonty):
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run("raise ValueError('test')")
    assert repr(exc_info.value) == snapshot('MontyRuntimeError(ValueError: test)')


def test_syntax_error_repr(monty_run: RunMonty):
    with pytest.raises(MontySyntaxError) as exc_info:
        monty_run('def')
    assert repr(exc_info.value) == snapshot('MontySyntaxError(Expected an identifier)')


def test_frame_repr(monty_run: RunMonty):
    code = """
def foo():
    raise ValueError('test')

foo()
"""
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run(code)
    frame = exc_info.value.traceback()[0]
    assert repr(frame) == snapshot("Frame(filename='<python-input-0>', line=5, column=1, function_name='<module>')")


def test_non_ascii_earlier_line_does_not_shift_columns(monty_run: RunMonty):
    # CodeRange stores raw byte offsets and the SourceMap expands them lazily,
    # so a multi-byte character on an earlier line must not shift the column
    # reported for a later line. Columns are characters, not bytes — the non-
    # ASCII slow path in SourceMap::resolve_byte is the interesting code here.
    code = "greeting = 'héllo'\nundefined_name\n"
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run(code)
    frames = exc_info.value.traceback()
    assert [f.dict() for f in frames] == snapshot(
        [
            {
                'filename': '<python-input-0>',
                'line': 2,
                'column': 1,
                'end_line': 2,
                'end_column': 15,
                'function_name': '<module>',
                'source_line': 'undefined_name',
            }
        ]
    )
