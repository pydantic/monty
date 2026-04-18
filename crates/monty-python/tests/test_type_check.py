import asyncio

import pytest
from inline_snapshot import snapshot

import pydantic_monty


def test_type_check_no_errors():
    """Type checking code with no errors returns None."""
    m = pydantic_monty.Monty('x = 1')
    assert m.type_check() is None


def test_type_check_no_cross_call_state_leak():
    """Successive calls must not see stale results from earlier calls.

    The type checker reuses warm `MemoryDb` instances from a pool, but each call must
    still scrub its root files before the db is returned. This regression test checks
    that a pooled db reused for the same script name does not leak the previous result.
    """
    # Valid code first.
    assert pydantic_monty.Monty('x = 1').type_check() is None
    # Same script path, invalid code — must produce a fresh error, not a cached None.
    with pytest.raises(pydantic_monty.MontyTypingError):
        pydantic_monty.Monty('"hello" + 1').type_check()
    # Back to valid code — must be None again, not a stale error.
    assert pydantic_monty.Monty('x = 1').type_check() is None


def test_type_check_stubs_not_leaked_to_later_call():
    """Stub declarations from an earlier call must not be visible to a later one.

    Warm pooled databases keep their semantic caches, but every call must delete and
    sync the temporary root files it wrote. A name defined in call 1's stubs must
    therefore be unresolved in call 2 when that call does not pass stubs.
    """
    # Call 1: prefix code declares `call1_stub_var`; code referencing it type-checks clean.
    assert pydantic_monty.Monty('result = call1_stub_var + 1').type_check(prefix_code='call1_stub_var = 0') is None
    # Call 2: same expression, no stubs — `call1_stub_var` must be undefined.
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        pydantic_monty.Monty('result = call1_stub_var + 1').type_check()
    assert str(exc_info.value) == snapshot("""\
error[unresolved-reference]: Name `call1_stub_var` used when not defined
 --> main.py:1:10
  |
1 | result = call1_stub_var + 1
  |          ^^^^^^^^^^^^^^
  |
info: rule `unresolved-reference` is enabled by default

""")


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
        script_name='agent.py',
        type_check=True,
        type_check_stubs=type_definitions,
    )

    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        pydantic_monty.Monty(
            code.replace('Messages', 'MXessages'),
            inputs=['prompt'],
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


# === MontyRepl type checking ===


def test_repl_type_check_method_no_errors():
    """MontyRepl.type_check() returns None for valid code."""
    repl = pydantic_monty.MontyRepl()
    assert repl.type_check('x = 1') is None


def test_repl_type_check_method_with_errors():
    """MontyRepl.type_check() raises MontyTypingError for invalid code."""
    repl = pydantic_monty.MontyRepl()
    with pytest.raises(pydantic_monty.MontyTypingError):
        repl.type_check('"hello" + 1')


def test_repl_type_check_method_with_prefix():
    """MontyRepl.type_check() uses prefix_code for declarations."""
    repl = pydantic_monty.MontyRepl()
    # Without prefix, x is undefined
    with pytest.raises(pydantic_monty.MontyTypingError):
        repl.type_check('result = x + 1')
    # With prefix declaring x, it should pass
    assert repl.type_check('result = x + 1', prefix_code='x = 0') is None


def test_repl_type_check_default_off():
    """Default type_check=False does not check code on feed_run."""
    repl = pydantic_monty.MontyRepl()
    # This has a type error but should not raise since type_check is off
    repl.feed_run('x = 1')


def test_repl_feed_run_type_check_enabled():
    """type_check=True raises MontyTypingError on feed_run with bad code."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        repl.feed_run('"hello" + 1')
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


def test_repl_feed_run_type_check_valid():
    """type_check=True allows valid code through."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    result = repl.feed_run('1 + 2')
    assert result == 3


def test_repl_type_check_accumulated():
    """Second snippet can see definitions from the first via accumulated code."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    repl.feed_run('x: int = 1')
    # Second snippet uses x — should pass because accumulated code defines it
    result = repl.feed_run('x + 2')
    assert result == 3


def test_repl_type_check_accumulated_function():
    """Functions defined in earlier snippets are visible to type checker."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    repl.feed_run("""
def add(a: int, b: int) -> int:
    return a + b
""")
    result = repl.feed_run('add(1, 2)')
    assert result == 3


def test_repl_type_check_with_stubs():
    """type_check_stubs provides context for type checking."""
    repl = pydantic_monty.MontyRepl(type_check=True, type_check_stubs='x: int = 0')
    # x is declared in stubs, so this should type-check fine
    result = repl.feed_run('x + 1', inputs={'x': 5})
    assert result == 6


def test_repl_skip_type_check():
    """skip_type_check=True bypasses type checking for that call."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    # Without skip, this raises MontyTypingError
    with pytest.raises(pydantic_monty.MontyTypingError):
        repl.feed_run('"hello" + 1')
    # With skip_type_check=True, the type error is not raised (but runtime error still occurs)
    with pytest.raises(pydantic_monty.MontyRuntimeError):
        repl.feed_run('"hello" + 1', skip_type_check=True)


def test_repl_type_check_line_numbers():
    """Error line numbers refer to the new snippet, not accumulated code."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    repl.feed_run('x: int = 1')
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        repl.feed_run('"hello" + 1')
    # Line 1 should refer to the new snippet, not offset by previous code
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


def test_repl_type_check_line_numbers_multiline():
    """Error line numbers are correct for multi-line snippets with accumulated context."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    repl.feed_run('x: int = 1')
    repl.feed_run('y: str = "hello"')
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        repl.feed_run('a = 1\nb = "hi" + 1')
    # Error is on line 2 of the new snippet, not offset by previous snippets
    assert str(exc_info.value) == snapshot("""\
error[unsupported-operator]: Unsupported `+` operation
 --> main.py:2:5
  |
1 | a = 1
2 | b = "hi" + 1
  |     ----^^^-
  |     |      |
  |     |      Has type `Literal[1]`
  |     Has type `Literal["hi"]`
  |
info: rule `unsupported-operator` is enabled by default

""")


def test_repl_type_check_stubs_with_external_functions():
    """type_check_stubs can declare external function signatures for the REPL."""
    stubs = """\
def fetch(url: str) -> str:
    return ''
"""
    repl = pydantic_monty.MontyRepl(type_check=True, type_check_stubs=stubs)
    # Should type-check fine: fetch is declared in stubs
    result = repl.feed_run(
        'result = fetch("https://example.com")',
        external_functions={'fetch': lambda url: 'response'},  # pyright: ignore[reportUnknownLambdaType]
    )
    assert result == snapshot(None)


def test_repl_type_check_stubs_wrong_arg_type():
    """type_check_stubs catches wrong argument types to declared functions."""
    stubs = """\
def fetch(url: str) -> str:
    return ''
"""
    repl = pydantic_monty.MontyRepl(type_check=True, type_check_stubs=stubs)
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        repl.feed_run('fetch(123)')
    assert str(exc_info.value) == snapshot("""\
error[invalid-argument-type]: Argument to function `fetch` is incorrect
 --> main.py:1:7
  |
1 | fetch(123)
  |       ^^^ Expected `str`, found `Literal[123]`
  |
info: Function defined here
 --> repl_type_stubs.pyi:1:5
  |
1 | def fetch(url: str) -> str:
  |     ^^^^^ -------- Parameter declared here
2 |     return ''
  |
info: rule `invalid-argument-type` is enabled by default

""")


def test_repl_type_check_accumulated_catches_type_mismatch():
    """Type checker catches mismatches with variables from earlier snippets."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    repl.feed_run('x: int = 1')
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        repl.feed_run('y: str = x')
    assert str(exc_info.value) == snapshot("""\
error[invalid-assignment]: Object of type `int` is not assignable to `str`
 --> main.py:1:4
  |
1 | y: str = x
  |    ---   ^ Incompatible value of type `int`
  |    |
  |    Declared type
  |
info: rule `invalid-assignment` is enabled by default

""")


def test_repl_type_check_feed_start():
    """feed_start also type-checks when type_check=True."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    with pytest.raises(pydantic_monty.MontyTypingError):
        repl.feed_start('"hello" + 1')


def test_repl_type_check_feed_start_valid():
    """feed_start allows valid code when type_check=True."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    progress = repl.feed_start('1 + 2')
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output == snapshot(3)


def test_repl_type_check_feed_start_skip():
    """feed_start respects skip_type_check."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    # Would fail type check, but skip_type_check=True bypasses it
    with pytest.raises(pydantic_monty.MontyRuntimeError):
        repl.feed_start('"hello" + 1', skip_type_check=True)


def test_repl_type_check_feed_start_accumulated():
    """feed_start sees accumulated code from prior feed_run calls."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    repl.feed_run('x: int = 10')
    progress = repl.feed_start('x + 5')
    assert isinstance(progress, pydantic_monty.MontyComplete)
    assert progress.output == snapshot(15)


def test_repl_type_check_display_format():
    """MontyTypingError from REPL type checking supports display() formats."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        repl.feed_run('"hello" + 1')
    assert exc_info.value.display('concise') == snapshot(
        'main.py:1:1: error[unsupported-operator] Operator `+` is not supported between objects of type `Literal["hello"]` and `Literal[1]`\n'
    )


def test_repl_type_check_stubs_filename():
    """Errors referencing stubs use the repl_type_stubs.pyi filename."""
    stubs = """\
def fetch(url: str) -> str:
    return ''
"""
    repl = pydantic_monty.MontyRepl(type_check=True, type_check_stubs=stubs)
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        repl.feed_run('fetch(123)')
    # The stubs file should be referenced as repl_type_stubs.pyi
    assert str(exc_info.value) == snapshot("""\
error[invalid-argument-type]: Argument to function `fetch` is incorrect
 --> main.py:1:7
  |
1 | fetch(123)
  |       ^^^ Expected `str`, found `Literal[123]`
  |
info: Function defined here
 --> repl_type_stubs.pyi:1:5
  |
1 | def fetch(url: str) -> str:
  |     ^^^^^ -------- Parameter declared here
2 |     return ''
  |
info: rule `invalid-argument-type` is enabled by default

""")


def test_repl_type_check_multiple_snippets_sequence():
    """Type checking works correctly across a sequence of snippets."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    repl.feed_run('x: int = 1')
    repl.feed_run('y: int = x + 1')
    repl.feed_run('z: int = x + y')
    result = repl.feed_run('x + y + z')
    assert result == snapshot(6)


def test_repl_type_check_function_define_then_call():
    """Function defined in one snippet can be called with correct types in the next."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    repl.feed_run("""\
def greet(name: str) -> str:
    return 'hello ' + name
""")
    result = repl.feed_run("greet('world')")
    assert result == snapshot('hello world')


def test_repl_type_check_function_define_then_call_wrong_type():
    """Calling a function from a prior snippet with wrong arg type is caught."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    repl.feed_run("""\
def greet(name: str) -> str:
    return 'hello ' + name
""")
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        repl.feed_run('greet(42)')
    assert str(exc_info.value) == snapshot("""\
error[invalid-argument-type]: Argument to function `greet` is incorrect
 --> main.py:1:7
  |
1 | greet(42)
  |       ^^ Expected `str`, found `Literal[42]`
  |
info: Function defined here
 --> repl_type_stubs.pyi:2:5
  |
2 | def greet(name: str) -> str:
  |     ^^^^^ --------- Parameter declared here
3 |     return 'hello ' + name
  |
info: rule `invalid-argument-type` is enabled by default

""")


def test_repl_type_check_function_return_type_used():
    """Return type of a function from a prior snippet is used for type checking."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    repl.feed_run("""\
def get_count() -> int:
    return 5
""")
    # Assigning return value to int should pass
    repl.feed_run('x: int = get_count()')
    # Assigning return value to str should fail
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        repl.feed_run('y: str = get_count()')
    assert str(exc_info.value) == snapshot("""\
error[invalid-assignment]: Object of type `int` is not assignable to `str`
 --> main.py:1:4
  |
1 | y: str = get_count()
  |    ---   ^^^^^^^^^^^ Incompatible value of type `int`
  |    |
  |    Declared type
  |
info: rule `invalid-assignment` is enabled by default

""")


def test_repl_type_check_redefine_function():
    """Redefining a function with a new signature updates the type checker's view."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    # First definition: takes int
    repl.feed_run("""\
def process(x: int) -> int:
    return x + 1
""")
    result = repl.feed_run('process(5)')
    assert result == snapshot(6)
    # Redefine: now takes str
    repl.feed_run("""\
def process(x: str) -> str:
    return x + '!'
""")
    result = repl.feed_run("process('hi')")
    assert result == snapshot('hi!')


def test_repl_type_check_redefine_function_then_call_later():
    """Redefining a function in one step, then calling it in a later step uses the new signature."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    # First definition: int -> int
    repl.feed_run("""\
def transform(x: int) -> int:
    return x + 1
""")
    assert repl.feed_run('transform(5)') == snapshot(6)
    # Redefine: str -> str
    repl.feed_run("""\
def transform(x: str) -> str:
    return x + '!'
""")
    # Call in a separate step — the accumulated stubs contain both definitions,
    # but the type checker should use the latest (str -> str)
    assert repl.feed_run("transform('hi')") == snapshot('hi!')
    # Calling with the old signature (int) should now fail type checking
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        repl.feed_run('transform(42)')
    assert str(exc_info.value) == snapshot("""\
error[invalid-argument-type]: Argument to function `transform` is incorrect
 --> main.py:1:11
  |
1 | transform(42)
  |           ^^ Expected `str`, found `Literal[42]`
  |
info: Function defined here
 --> repl_type_stubs.pyi:6:5
  |
5 | transform(5)
6 | def transform(x: str) -> str:
  |     ^^^^^^^^^ ------ Parameter declared here
7 |     return x + '!'
  |
info: rule `invalid-argument-type` is enabled by default

""")


def test_repl_type_check_redefine_variable_type():
    """Redefining a variable with a new type updates the type checker's view."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    repl.feed_run('x: int = 1')
    repl.feed_run('y: int = x + 1')
    assert repl.feed_run('y') == snapshot(2)
    # Redefine x as str
    repl.feed_run('x: str = "hello"')
    result = repl.feed_run('x + " world"')
    assert result == snapshot('hello world')


def test_repl_type_check_function_calling_prior_function():
    """A function defined in one snippet can call a function from an earlier snippet."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    repl.feed_run("""\
def double(x: int) -> int:
    return x * 2
""")
    repl.feed_run("""\
def quadruple(x: int) -> int:
    return double(double(x))
""")
    result = repl.feed_run('quadruple(3)')
    assert result == snapshot(12)


def test_repl_type_check_variable_used_across_many_snippets():
    """A variable defined early is usable across many subsequent snippets."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    repl.feed_run('total: int = 0')
    repl.feed_run('total = total + 10')
    repl.feed_run('total = total + 20')
    repl.feed_run('total = total + 30')
    result = repl.feed_run('total')
    assert result == snapshot(60)


def test_repl_type_check_stubs_and_accumulated_together():
    """Stubs and accumulated code both contribute to type checking context."""
    stubs = """\
def multiply(a: int, b: int) -> int:
    return 0
"""
    repl = pydantic_monty.MontyRepl(type_check=True, type_check_stubs=stubs)
    # Define a helper that references the stub function — type checking passes
    # because the stub declares multiply. skip_type_check on subsequent calls
    # since multiply doesn't exist at runtime.
    repl.feed_run("""\
def square(x: int) -> int:
    return multiply(x, x)
""")
    # Type checker sees square returns int (from accumulated) and multiply takes ints (from stubs)
    # Calling square with wrong type should fail type checking
    with pytest.raises(pydantic_monty.MontyTypingError):
        repl.feed_run('square("hello")')
    # Assigning square's return to wrong type should also fail
    with pytest.raises(pydantic_monty.MontyTypingError):
        repl.feed_run('bad: str = square(5)')


def test_repl_type_check_multiple_functions_interacting():
    """Multiple functions defined across snippets can interact correctly."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    repl.feed_run("""\
def to_int(s: str) -> int:
    return len(s)
""")
    repl.feed_run("""\
def to_str(n: int) -> str:
    return str(n)
""")
    repl.feed_run("""\
def roundtrip(s: str) -> str:
    return to_str(to_int(s))
""")
    result = repl.feed_run("roundtrip('hello')")
    assert result == snapshot('5')


def test_repl_type_check_script_name():
    """Custom script_name appears in type check error messages."""
    repl = pydantic_monty.MontyRepl(type_check=True, script_name='my_repl.py')
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        repl.feed_run('"hello" + 1')
    assert str(exc_info.value) == snapshot("""\
error[unsupported-operator]: Unsupported `+` operation
 --> my_repl.py:1:1
  |
1 | "hello" + 1
  | -------^^^-
  | |         |
  | |         Has type `Literal[1]`
  | Has type `Literal["hello"]`
  |
info: rule `unsupported-operator` is enabled by default

""")


def test_repl_type_check_dump_load_preserves_state():
    """Type checking state is preserved through dump/load."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    repl.feed_run('x: int = 1')

    data = repl.dump()
    repl2 = pydantic_monty.MontyRepl.load(data)

    # Loaded REPL should still type-check with accumulated context
    result = repl2.feed_run('x + 2')
    assert result == snapshot(3)

    # And still catch type errors
    with pytest.raises(pydantic_monty.MontyTypingError):
        repl2.feed_run('"hello" + 1')


def test_repl_type_check_feed_start_runtime_error_does_not_pollute_state():
    """A failed feed_start() snippet must not leak definitions into later type checks."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    with pytest.raises(pydantic_monty.MontyRuntimeError):
        repl.feed_start("""\
def foo(x: int) -> int:
    return x

1 / 0
""")

    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        repl.feed_run('foo("x")')
    assert str(exc_info.value) == snapshot("""\
error[unresolved-reference]: Name `foo` used when not defined
 --> main.py:1:1
  |
1 | foo("x")
  | ^^^
  |
info: rule `unresolved-reference` is enabled by default

""")


def test_repl_type_check_load_repl_snapshot_preserves_accumulated_state():
    """load_repl_snapshot() keeps prior committed type-check state."""
    repl = pydantic_monty.MontyRepl(
        type_check=True,
        type_check_stubs="""\
def fetch(x: int) -> int:
    return 0
""",
    )
    repl.feed_run('x: int = 1')

    progress = repl.feed_start('fetch(x)')
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    data = progress.dump()
    loaded, loaded_repl = pydantic_monty.load_repl_snapshot(data)
    assert isinstance(loaded, pydantic_monty.FunctionSnapshot)
    loaded.resume({'return_value': 1})

    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        loaded_repl.feed_run('y: str = x')
    assert str(exc_info.value) == snapshot("""\
error[invalid-assignment]: Object of type `int` is not assignable to `str`
 --> main.py:1:4
  |
1 | y: str = x
  |    ---   ^ Incompatible value of type `int`
  |    |
  |    Declared type
  |
info: rule `invalid-assignment` is enabled by default

""")


def test_repl_type_check_load_repl_snapshot_preserves_pending_snippet():
    """A paused feed_start() snippet becomes visible to type checking after snapshot completion."""
    repl = pydantic_monty.MontyRepl(
        type_check=True,
        type_check_stubs="""\
def fetch(x: int) -> int:
    return 0
""",
    )

    progress = repl.feed_start("""\
def foo(x: int) -> int:
    return x

fetch(1)
""")
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    data = progress.dump()
    loaded, loaded_repl = pydantic_monty.load_repl_snapshot(data)
    assert isinstance(loaded, pydantic_monty.FunctionSnapshot)
    loaded.resume({'return_value': 1})

    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        loaded_repl.feed_run('foo("x")')
    assert str(exc_info.value) == snapshot("""\
error[invalid-argument-type]: Argument to function `foo` is incorrect
 --> main.py:1:5
  |
1 | foo("x")
  |     ^^^ Expected `int`, found `Literal["x"]`
  |
info: Function defined here
 --> repl_type_stubs.pyi:4:5
  |
2 |     return 0
3 |
4 | def foo(x: int) -> int:
  |     ^^^ ------ Parameter declared here
5 |     return x
  |
info: rule `invalid-argument-type` is enabled by default

""")


def test_repl_type_check_load_repl_snapshot_preserves_user_stubs():
    """load_repl_snapshot() preserves user-provided type_check_stubs."""
    repl = pydantic_monty.MontyRepl(
        type_check=True,
        type_check_stubs="""\
def fetch(url: str) -> str:
    return ''
""",
    )

    progress = repl.feed_start('fetch("https://example.com")')
    assert isinstance(progress, pydantic_monty.FunctionSnapshot)
    data = progress.dump()
    loaded, loaded_repl = pydantic_monty.load_repl_snapshot(data)
    assert isinstance(loaded, pydantic_monty.FunctionSnapshot)
    loaded.resume({'return_value': 'ok'})

    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        loaded_repl.feed_run('fetch(123)')
    assert str(exc_info.value) == snapshot("""\
error[invalid-argument-type]: Argument to function `fetch` is incorrect
 --> main.py:1:7
  |
1 | fetch(123)
  |       ^^^ Expected `str`, found `Literal[123]`
  |
info: Function defined here
 --> repl_type_stubs.pyi:1:5
  |
1 | def fetch(url: str) -> str:
  |     ^^^^^ -------- Parameter declared here
2 |     return ''
  |
info: rule `invalid-argument-type` is enabled by default

""")


def test_repl_type_check_stubs_without_trailing_newline():
    """Stubs without a trailing newline don't corrupt accumulated code."""
    stubs = 'def fetch(url: str) -> str: ...'  # no trailing \n
    repl = pydantic_monty.MontyRepl(type_check=True, type_check_stubs=stubs)
    repl.feed_run(
        "response = fetch('url')",
        external_functions={'fetch': lambda url: 'data'},  # pyright: ignore[reportUnknownLambdaType]
    )
    # response must be visible in the next snippet even though stubs lacked \n
    result = repl.feed_run('response.upper()')
    assert result == snapshot('DATA')


# === skip_type_check does not append to accumulated stubs ===


def test_repl_type_check_feed_run_skip_does_not_accumulate():
    """skip_type_check=True on feed_run bypasses the check AND does not add the snippet to accumulated stubs."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    # Run a snippet with skip — it must not leak into later type checks
    repl.feed_run('x = "hello"', skip_type_check=True)
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        repl.feed_run('x + 1')
    assert str(exc_info.value) == snapshot("""\
error[unresolved-reference]: Name `x` used when not defined
 --> main.py:1:1
  |
1 | x + 1
  | ^
  |
info: rule `unresolved-reference` is enabled by default

""")


def test_repl_type_check_feed_start_skip_does_not_accumulate():
    """skip_type_check=True on feed_start does not add the snippet to accumulated stubs."""
    repl = pydantic_monty.MontyRepl(type_check=True)
    progress = repl.feed_start('x = 42', skip_type_check=True)
    assert isinstance(progress, pydantic_monty.MontyComplete)
    # x should NOT be visible to subsequent type-checked snippets
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        repl.feed_run('x + 1')
    assert str(exc_info.value) == snapshot("""\
error[unresolved-reference]: Name `x` used when not defined
 --> main.py:1:1
  |
1 | x + 1
  | ^
  |
info: rule `unresolved-reference` is enabled by default

""")


# === feed_run_async type check ===


def test_repl_type_check_feed_run_async_enabled():
    """type_check=True raises MontyTypingError on feed_run_async with bad code."""
    repl = pydantic_monty.MontyRepl(type_check=True)

    async def go():
        with pytest.raises(pydantic_monty.MontyTypingError):
            await repl.feed_run_async('"hello" + 1')

    asyncio.run(go())


def test_repl_type_check_feed_run_async_valid():
    """type_check=True allows valid code through feed_run_async."""
    repl = pydantic_monty.MontyRepl(type_check=True)

    async def go():
        return await repl.feed_run_async('1 + 2')

    assert asyncio.run(go()) == snapshot(3)


def test_repl_type_check_feed_run_async_skip():
    """feed_run_async respects skip_type_check."""
    repl = pydantic_monty.MontyRepl(type_check=True)

    async def go():
        # Would fail type check, but skip_type_check=True bypasses it
        with pytest.raises(pydantic_monty.MontyRuntimeError):
            await repl.feed_run_async('"hello" + 1', skip_type_check=True)

    asyncio.run(go())


def test_repl_type_check_feed_run_async_skip_does_not_accumulate():
    """skip_type_check=True on feed_run_async does not add the snippet to accumulated stubs."""
    repl = pydantic_monty.MontyRepl(type_check=True)

    async def go():
        await repl.feed_run_async('x = 42', skip_type_check=True)

    asyncio.run(go())
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        repl.feed_run('x + 1')
    assert str(exc_info.value) == snapshot("""\
error[unresolved-reference]: Name `x` used when not defined
 --> main.py:1:1
  |
1 | x + 1
  | ^
  |
info: rule `unresolved-reference` is enabled by default

""")


def test_repl_type_check_feed_run_async_runtime_error_does_not_pollute_state():
    """A failed feed_run_async() snippet must not leak definitions into later type checks."""
    repl = pydantic_monty.MontyRepl(type_check=True)

    async def go():
        with pytest.raises(pydantic_monty.MontyRuntimeError):
            await repl.feed_run_async("""\
def foo(x: int) -> int:
    return x

1 / 0
""")

    asyncio.run(go())
    with pytest.raises(pydantic_monty.MontyTypingError) as exc_info:
        repl.feed_run('foo("x")')
    assert str(exc_info.value) == snapshot("""\
error[unresolved-reference]: Name `foo` used when not defined
 --> main.py:1:1
  |
1 | foo("x")
  | ^^^
  |
info: rule `unresolved-reference` is enabled by default

""")
