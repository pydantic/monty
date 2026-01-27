import pytest
from inline_snapshot import snapshot

import pydantic_monty


def test_type_check_no_errors():
    """Type checking code with no errors returns None."""
    m = pydantic_monty.Monty('x = 1')
    assert m.type_check() is None


def test_type_check_with_errors():
    """Type checking code with type errors raises MontyTypingError."""
    m = pydantic_monty.Monty('"hello" + 1')
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
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
    m = pydantic_monty.Monty(code)
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
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
    m = pydantic_monty.Monty('print(undefined_var)')
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
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
    m = pydantic_monty.Monty(code)
    assert m.type_check() is None


def test_type_check_with_prefix_code():
    """Type checking with prefix code for input declarations."""
    m = pydantic_monty.Monty('result = x + 1')
    # Without prefix, x is undefined
    with pytest.raises(pydantic_monty.MontyTypingError):
        m.type_check()
    # With prefix declaring x as a variable, it should pass
    assert m.type_check(prefix_code='x = 0') is None


def test_type_check_display_invalid_format():
    """Invalid format string on display() raises ValueError."""
    m = pydantic_monty.Monty('"hello" + 1')
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        m.type_check()
    with pytest.raises(ValueError) as val_exc:
        exc_info.value.display('invalid_format')  # pyright: ignore[reportArgumentType]
    assert str(val_exc.value) == snapshot('Unknown format: invalid_format')


def test_type_check_display_concise_format():
    """Type checking with concise format via display()."""
    m = pydantic_monty.Monty('"hello" + 1')
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        m.type_check()
    assert exc_info.value.display('concise') == snapshot(
        'main.py:1:1: error[unsupported-operator] Operator `+` is not supported between objects of type `Literal["hello"]` and `Literal[1]`\n'
    )


# === MontyTypingError tests ===


def test_monty_typing_error_is_monty_error_subclass():
    """MontyTypingError is a subclass of MontyError."""
    m = pydantic_monty.Monty('"hello" + 1')
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        m.type_check()
    error = exc_info.value
    assert isinstance(error, pydantic_monty.MontyError)
    assert isinstance(error, Exception)


def test_monty_typing_error_repr():
    """MontyTypingError has proper repr with truncation."""
    m = pydantic_monty.Monty('"hello" + 1')
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
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
    m = pydantic_monty.Monty('"hello" + 1')
    with pytest.raises(pydantic_monty.MontyError):
        m.type_check()


def test_monty_typing_error_display_default():
    """MontyTypingError display() defaults to full format."""
    m = pydantic_monty.Monty('"hello" + 1')
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        m.type_check()
    # Default display should match str()
    assert exc_info.value.display() == str(exc_info.value)


# === Constructor type_check parameter tests ===


def test_constructor_type_check_default_false():
    """Type checking is disabled by default in constructor."""
    # This should NOT raise during construction (type_check=False is default)
    m = pydantic_monty.Monty('"hello" + 1')
    # But we can still call type_check() manually later
    with pytest.raises(pydantic_monty.MontyTypingError):
        m.type_check()


def test_constructor_type_check_explicit_true():
    """Explicit type_check=True raises on type errors."""
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        pydantic_monty.Monty('"hello" + 1', type_check=True)
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


def test_constructor_type_check_explicit_false():
    """Explicit type_check=False skips type checking during construction."""
    # This should NOT raise during construction
    m = pydantic_monty.Monty('"hello" + 1', type_check=False)
    # But we can still call type_check() manually later
    with pytest.raises(pydantic_monty.MontyTypingError):
        m.type_check()


def test_constructor_default_allows_run_with_inputs():
    """Default (type_check=False) allows running code that would fail type checking."""
    # Code with undefined variable - type checking would fail
    m = pydantic_monty.Monty('x + 1', inputs=['x'])
    # But runtime works fine with the input provided
    result = m.run(inputs={'x': 5})
    assert result == 6


def test_constructor_type_check_stubs():
    """type_check_stubs provides declarations for type checking."""
    # Without prefix, this would fail type checking (x is undefined)
    # Use assignment to define x, not just type annotation
    m = pydantic_monty.Monty('result = x + 1', type_check=True, type_check_stubs='x = 0')
    # Should construct successfully because prefix declares x
    assert m is not None


def test_constructor_type_check_stubs_with_external_function():
    """type_check_stubs can declare external function signatures."""
    # Define fetch as a function that takes a string and returns a string
    prefix = """
def fetch(url: str) -> str:
    return ''
"""
    m = pydantic_monty.Monty(
        'result = fetch("https://example.com")',
        external_functions=['fetch'],
        type_check=True,
        type_check_stubs=prefix,
    )
    assert m is not None


def test_constructor_type_check_stubs_invalid():
    """type_check_stubs with wrong types still catches errors."""
    # Prefix defines x as str, but code tries to use it with int addition
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        pydantic_monty.Monty(
            'result: int = x + 1',
            type_check=True,
            type_check_stubs='x = "hello"',
        )
    # Should fail because str + int is invalid
    assert str(exc_info.value) == snapshot("""\
error[unsupported-operator]: Unsupported `+` operation
 --> main.py:1:15
  |
1 | result: int = x + 1
  |               -^^^-
  |               |   |
  |               |   Has type `Literal[1]`
  |               Has type `Literal["hello"]`
  |
info: rule `unsupported-operator` is enabled by default

""")


def test_inject_stubs_offset():
    type_definitions = """\
from typing import Any

Messages = list[dict[str, Any]]

async def call_llm(prompt: str, messages: Messages) -> str | Messages:
    ...

prompt: str = ''
"""

    code = """\
async def agent(prompt: str, messages: Messages):
    while True:
        print(f'messages so far: {messages}')
        output = await call_llm(prompt, messages)
        if isinstance(output, str):
            return output
        messages.extend(output)

await agent(prompt, [])
"""
    pydantic_monty.Monty(
        code,
        inputs=['prompt'],
        external_functions=['call_llm'],
        script_name='agent.py',
        type_check=True,
        type_check_stubs=type_definitions,
    )

    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        pydantic_monty.Monty(
            code.replace('Messages', 'MXessages'),
            inputs=['prompt'],
            external_functions=['call_llm'],
            script_name='agent.py',
            type_check=True,
            type_check_stubs=type_definitions,
        )
    assert str(exc_info.value) == snapshot("""\
error[unresolved-reference]: Name `MXessages` used when not defined
 --> agent.py:1:40
  |
1 | async def agent(prompt: str, messages: MXessages):
  |                                        ^^^^^^^^^
2 |     while True:
3 |         print(f'messages so far: {messages}')
  |
info: rule `unresolved-reference` is enabled by default

""")

    code_call_func_wrong = 'await call_llm(prompt, 42)'

    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        pydantic_monty.Monty(
            code_call_func_wrong,
            inputs=['prompt'],
            external_functions=['call_llm'],
            script_name='agent.py',
            type_check=True,
            type_check_stubs=type_definitions,
        )
    assert str(exc_info.value) == snapshot("""\
error[invalid-argument-type]: Argument to function `call_llm` is incorrect
 --> agent.py:1:24
  |
1 | await call_llm(prompt, 42)
  |                        ^^ Expected `list[dict[str, Any]]`, found `Literal[42]`
  |
info: Function defined here
 --> type_stubs.pyi:5:11
  |
3 | Messages = list[dict[str, Any]]
4 |
5 | async def call_llm(prompt: str, messages: Messages) -> str | Messages:
  |           ^^^^^^^^              ------------------ Parameter declared here
6 |     ...
  |
info: rule `invalid-argument-type` is enabled by default

""")
