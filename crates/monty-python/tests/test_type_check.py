"""Type-check tests: `pool.checkout(type_check=True)` runs ty inside the worker.

Each successfully executed snippet accumulates into the type-check context used
for subsequent feeds; failures raise `MontyTypingError` with pre-rendered text.
"""

from __future__ import annotations

from collections.abc import Iterator

import pytest
from inline_snapshot import snapshot

from pydantic_monty import Monty, MontyError, MontyRuntimeError, MontySession, MontyTypingError


@pytest.fixture
def tc_session(pool: Monty) -> Iterator[MontySession]:
    """A fresh session with type checking enabled."""
    with pool.checkout(type_check=True) as s:
        yield s


# === Basic type checking ===


def test_type_check_no_errors(tc_session: MontySession):
    """Valid code passes type checking and executes."""
    assert tc_session.feed_run('1 + 2') == 3


def test_type_check_with_errors(tc_session: MontySession):
    """Code with type errors raises MontyTypingError."""
    with pytest.raises(MontyTypingError) as exc_info:
        tc_session.feed_run('"hello" + 1')
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

""")


def test_type_check_no_cross_session_state_leak(pool: Monty):
    """A later checkout must not see stale results from an earlier one."""
    # Valid code first.
    with pool.checkout(type_check=True) as session:
        session.feed_run('x = 1')
    # Invalid code — must produce a fresh error, not a cached pass.
    with pool.checkout(type_check=True) as session:
        with pytest.raises(MontyTypingError):
            session.feed_run('"hello" + 1')
    # Back to valid code — must pass again, not report a stale error.
    with pool.checkout(type_check=True) as session:
        session.feed_run('x = 1')


def test_type_check_stubs_not_leaked_to_later_session(pool: Monty):
    """Stub declarations from one checkout must not be visible to a later one."""
    with pool.checkout(type_check=True, type_check_stubs='call1_stub_var = 0') as session:
        session.feed_run('result = call1_stub_var + 1', inputs={'call1_stub_var': 1})
    with pool.checkout(type_check=True) as session:
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run('result = call1_stub_var + 1')
        assert str(exc_info.value) == snapshot("""\
error[unresolved-reference]: Name `call1_stub_var` used when not defined
 --> main.py:1:10
  |
1 | result = call1_stub_var + 1
  |          ^^^^^^^^^^^^^^
  |

""")


def test_type_check_function_return_type(tc_session: MontySession):
    """Type checking detects mismatched return types."""
    code = """
def foo() -> int:
    return "not an int"
"""
    with pytest.raises(MontyTypingError) as exc_info:
        tc_session.feed_run(code)
    assert str(exc_info.value) == snapshot("""\
error[invalid-return-type]: Return type does not match returned value
 --> main.py:2:14
  |
2 | def foo() -> int:
  |              --- Expected `int` because of return type
3 |     return "not an int"
  |            ^^^^^^^^^^^^ expected `int`, found `Literal["not an int"]`
  |

""")


def test_type_check_undefined_variable(tc_session: MontySession):
    """Type checking detects undefined variables."""
    with pytest.raises(MontyTypingError) as exc_info:
        tc_session.feed_run('print(undefined_var)')
    assert str(exc_info.value) == snapshot("""\
error[unresolved-reference]: Name `undefined_var` used when not defined
 --> main.py:1:7
  |
1 | print(undefined_var)
  |       ^^^^^^^^^^^^^
  |

""")


def test_type_check_valid_function(tc_session: MontySession):
    """Type checking valid function passes."""
    code = """
def add(a: int, b: int) -> int:
    return a + b

add(1, 2)
"""
    assert tc_session.feed_run(code) == 3


def test_type_check_disabled_by_default(pool: Monty):
    """Without type_check=True, type errors surface at runtime only."""
    with pool.checkout() as session:
        with pytest.raises(MontyRuntimeError):
            session.feed_run('"hello" + 1')


def test_type_check_default_allows_run_with_inputs(pool: Monty):
    """Default (type_check=False) allows running code that would fail type checking."""
    with pool.checkout() as session:
        assert session.feed_run('x + 1', inputs={'x': 5}) == 6


# === MontyTypingError shape ===


def test_monty_typing_error_is_monty_error_subclass(tc_session: MontySession):
    """MontyTypingError is a subclass of MontyError."""
    with pytest.raises(MontyTypingError) as exc_info:
        tc_session.feed_run('"hello" + 1')
    error = exc_info.value
    assert isinstance(error, MontyError)
    assert isinstance(error, Exception)


def test_monty_typing_error_caught_as_monty_error(tc_session: MontySession):
    """MontyTypingError can be caught as MontyError."""
    with pytest.raises(MontyError):
        tc_session.feed_run('"hello" + 1')


def test_monty_typing_error_repr(tc_session: MontySession):
    with pytest.raises(MontyTypingError) as exc_info:
        tc_session.feed_run('"hello" + 1')
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

)\
""")


def test_monty_typing_error_display_matches_str(tc_session: MontySession):
    """display() takes no arguments and returns the same pre-rendered text as str()."""
    with pytest.raises(MontyTypingError) as exc_info:
        tc_session.feed_run('"hello" + 1')
    assert exc_info.value.display() == str(exc_info.value)


# === type_check_stubs ===


def test_type_check_stubs(pool: Monty):
    """type_check_stubs provides declarations for type checking."""
    with pool.checkout(type_check=True, type_check_stubs='x = 0') as session:
        # x is declared in stubs, so this type-checks; the input provides it at runtime
        assert session.feed_run('result = x + 1\nresult', inputs={'x': 5}) == 6


def test_type_check_stubs_with_external_function(pool: Monty):
    """type_check_stubs can declare external function signatures."""
    stubs = """\
def fetch(url: str) -> str:
    return ''
"""
    with pool.checkout(type_check=True, type_check_stubs=stubs) as session:
        result = session.feed_run(
            'result = fetch("https://example.com")',
            external_lookup={'fetch': lambda url: 'response'},  # pyright: ignore[reportUnknownLambdaType]
        )
        assert result == snapshot(None)


def test_type_check_stubs_wrong_arg_type(pool: Monty):
    """type_check_stubs catches wrong argument types to declared functions."""
    stubs = """\
def fetch(url: str) -> str:
    return ''
"""
    with pool.checkout(type_check=True, type_check_stubs=stubs) as session:
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run('fetch(123)')
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

""")


def test_type_check_stubs_invalid(pool: Monty):
    """Stub-declared types still catch errors in fed code."""
    with pool.checkout(type_check=True, type_check_stubs='x = "hello"') as session:
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run('result: int = x + 1')
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

""")


def test_type_check_stubs_without_trailing_newline(pool: Monty):
    """Stubs without a trailing newline don't corrupt accumulated code."""
    stubs = 'def fetch(url: str) -> str: ...'  # no trailing \n
    with pool.checkout(type_check=True, type_check_stubs=stubs) as session:
        session.feed_run(
            "response = fetch('url')",
            external_lookup={'fetch': lambda url: 'data'},  # pyright: ignore[reportUnknownLambdaType]
        )
        # response must be visible in the next snippet even though stubs lacked \n
        assert session.feed_run('response.upper()') == snapshot('DATA')


def test_inject_stubs_offset(pool: Monty):
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
    # the definition type-checks cleanly against the stubs (the awaiting call
    # itself needs an async session, so only the def is executed here)
    code_defs = code.rsplit('\n\n', 1)[0]
    with pool.checkout(script_name='agent.py', type_check=True, type_check_stubs=type_definitions) as session:
        assert session.feed_run(code_defs, inputs={'prompt': 'hi'}) is None

    with pool.checkout(script_name='agent.py', type_check=True, type_check_stubs=type_definitions) as session:
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run(code.replace('Messages', 'MXessages'), inputs={'prompt': 'hi'})
        assert str(exc_info.value) == snapshot("""\
error[unresolved-reference]: Name `MXessages` used when not defined
 --> agent.py:1:40
  |
1 | async def agent(prompt: str, messages: MXessages):
  |                                        ^^^^^^^^^
2 |     while True:
3 |         print(f'messages so far: {messages}')
  |

""")

    with pool.checkout(script_name='agent.py', type_check=True, type_check_stubs=type_definitions) as session:
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run('await call_llm(prompt, 42)', inputs={'prompt': 'hi'})
        assert str(exc_info.value) == snapshot("""\
error[invalid-argument-type]: Argument to function `call_llm` is incorrect
 --> agent.py:1:24
  |
1 | await call_llm(prompt, 42)
  |                        ^^ Expected `list[dict[str, Any]]`, found `Literal[42]`
  |
info: Function defined here
 --> repl_type_stubs.pyi:5:11
  |
3 | Messages = list[dict[str, Any]]
4 |
5 | async def call_llm(prompt: str, messages: Messages) -> str | Messages:
  |           ^^^^^^^^              ------------------ Parameter declared here
6 |     ...
  |

""")


# === Accumulated context across feeds ===


def test_type_check_accumulated(tc_session: MontySession):
    """Second snippet can see definitions from the first via accumulated code."""
    tc_session.feed_run('x: int = 1')
    assert tc_session.feed_run('x + 2') == 3


def test_type_check_accumulated_function(tc_session: MontySession):
    """Functions defined in earlier snippets are visible to the type checker."""
    tc_session.feed_run("""
def add(a: int, b: int) -> int:
    return a + b
""")
    assert tc_session.feed_run('add(1, 2)') == 3


def test_type_check_accumulated_catches_type_mismatch(tc_session: MontySession):
    """Type checker catches mismatches with variables from earlier snippets."""
    tc_session.feed_run('x: int = 1')
    with pytest.raises(MontyTypingError) as exc_info:
        tc_session.feed_run('y: str = x')
    assert str(exc_info.value) == snapshot("""\
error[invalid-assignment]: Object of type `int` is not assignable to `str`
 --> main.py:1:4
  |
1 | y: str = x
  |    ---   ^ Incompatible value of type `int`
  |    |
  |    Declared type
  |

""")


def test_type_check_line_numbers(tc_session: MontySession):
    """Error line numbers refer to the new snippet, not accumulated code."""
    tc_session.feed_run('x: int = 1')
    with pytest.raises(MontyTypingError) as exc_info:
        tc_session.feed_run('"hello" + 1')
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

""")


def test_type_check_line_numbers_multiline(tc_session: MontySession):
    """Error line numbers are correct for multi-line snippets with accumulated context."""
    tc_session.feed_run('x: int = 1')
    tc_session.feed_run('y: str = "hello"')
    with pytest.raises(MontyTypingError) as exc_info:
        tc_session.feed_run('a = 1\nb = "hi" + 1')
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

""")


def test_type_check_multiple_snippets_sequence(tc_session: MontySession):
    """Type checking works correctly across a sequence of snippets."""
    tc_session.feed_run('x: int = 1')
    tc_session.feed_run('y: int = x + 1')
    tc_session.feed_run('z: int = x + y')
    assert tc_session.feed_run('x + y + z') == snapshot(6)


def test_type_check_function_define_then_call(tc_session: MontySession):
    """Function defined in one snippet can be called with correct types in the next."""
    tc_session.feed_run("""\
def greet(name: str) -> str:
    return 'hello ' + name
""")
    assert tc_session.feed_run("greet('world')") == snapshot('hello world')


def test_type_check_function_define_then_call_wrong_type(tc_session: MontySession):
    """Calling a function from a prior snippet with wrong arg type is caught."""
    tc_session.feed_run("""\
def greet(name: str) -> str:
    return 'hello ' + name
""")
    with pytest.raises(MontyTypingError) as exc_info:
        tc_session.feed_run('greet(42)')
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

""")


def test_type_check_function_return_type_used(tc_session: MontySession):
    """Return type of a function from a prior snippet is used for type checking."""
    tc_session.feed_run("""\
def get_count() -> int:
    return 5
""")
    # Assigning return value to int should pass
    tc_session.feed_run('x: int = get_count()')
    # Assigning return value to str should fail
    with pytest.raises(MontyTypingError) as exc_info:
        tc_session.feed_run('y: str = get_count()')
    assert str(exc_info.value) == snapshot("""\
error[invalid-assignment]: Object of type `int` is not assignable to `str`
 --> main.py:1:4
  |
1 | y: str = get_count()
  |    ---   ^^^^^^^^^^^ Incompatible value of type `int`
  |    |
  |    Declared type
  |

""")


def test_type_check_redefine_function(tc_session: MontySession):
    """Redefining a function with a new signature updates the type checker's view."""
    # First definition: takes int
    tc_session.feed_run("""\
def process(x: int) -> int:
    return x + 1
""")
    assert tc_session.feed_run('process(5)') == snapshot(6)
    # Redefine: now takes str
    tc_session.feed_run("""\
def process(x: str) -> str:
    return x + '!'
""")
    assert tc_session.feed_run("process('hi')") == snapshot('hi!')


def test_type_check_redefine_function_then_call_later(tc_session: MontySession):
    """Redefining a function in one step, then calling it in a later step uses the new signature."""
    # First definition: int -> int
    tc_session.feed_run("""\
def transform(x: int) -> int:
    return x + 1
""")
    assert tc_session.feed_run('transform(5)') == snapshot(6)
    # Redefine: str -> str
    tc_session.feed_run("""\
def transform(x: str) -> str:
    return x + '!'
""")
    # Call in a separate step — the accumulated stubs contain both definitions,
    # but the type checker should use the latest (str -> str)
    assert tc_session.feed_run("transform('hi')") == snapshot('hi!')
    # Calling with the old signature (int) should now fail type checking
    with pytest.raises(MontyTypingError) as exc_info:
        tc_session.feed_run('transform(42)')
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

""")


def test_type_check_redefine_variable_type(tc_session: MontySession):
    """Redefining a variable with a new type updates the type checker's view."""
    tc_session.feed_run('x: int = 1')
    tc_session.feed_run('y: int = x + 1')
    assert tc_session.feed_run('y') == snapshot(2)
    # Redefine x as str
    tc_session.feed_run('x: str = "hello"')
    assert tc_session.feed_run('x + " world"') == snapshot('hello world')


def test_type_check_function_calling_prior_function(tc_session: MontySession):
    """A function defined in one snippet can call a function from an earlier snippet."""
    tc_session.feed_run("""\
def double(x: int) -> int:
    return x * 2
""")
    tc_session.feed_run("""\
def quadruple(x: int) -> int:
    return double(double(x))
""")
    assert tc_session.feed_run('quadruple(3)') == snapshot(12)


def test_type_check_variable_used_across_many_snippets(tc_session: MontySession):
    """A variable defined early is usable across many subsequent snippets."""
    tc_session.feed_run('total: int = 0')
    tc_session.feed_run('total = total + 10')
    tc_session.feed_run('total = total + 20')
    tc_session.feed_run('total = total + 30')
    assert tc_session.feed_run('total') == snapshot(60)


def test_type_check_stubs_and_accumulated_together(pool: Monty):
    """Stubs and accumulated code both contribute to type checking context."""
    stubs = """\
def multiply(a: int, b: int) -> int:
    return 0
"""
    with pool.checkout(type_check=True, type_check_stubs=stubs) as session:
        # Define a helper that references the stub function — type checking passes
        # because the stub declares multiply.
        session.feed_run("""\
def square(x: int) -> int:
    return multiply(x, x)
""")
        # Type checker sees square returns int (accumulated) and multiply takes ints (stubs):
        # calling square with the wrong type should fail type checking
        with pytest.raises(MontyTypingError):
            session.feed_run('square("hello")')
        # Assigning square's return to wrong type should also fail
        with pytest.raises(MontyTypingError):
            session.feed_run('bad: str = square(5)')


def test_type_check_multiple_functions_interacting(tc_session: MontySession):
    """Multiple functions defined across snippets can interact correctly."""
    tc_session.feed_run("""\
def to_int(s: str) -> int:
    return len(s)
""")
    tc_session.feed_run("""\
def to_str(n: int) -> str:
    return str(n)
""")
    tc_session.feed_run("""\
def roundtrip(s: str) -> str:
    return to_str(to_int(s))
""")
    assert tc_session.feed_run("roundtrip('hello')") == snapshot('5')


def test_type_check_script_name(pool: Monty):
    """Custom script_name appears in type check error messages."""
    with pool.checkout(type_check=True, script_name='my_repl.py') as session:
        with pytest.raises(MontyTypingError) as exc_info:
            session.feed_run('"hello" + 1')
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

""")


# === skip_type_check ===


def test_skip_type_check(tc_session: MontySession):
    """skip_type_check=True bypasses type checking for that call."""
    # Without skip, this raises MontyTypingError
    with pytest.raises(MontyTypingError):
        tc_session.feed_run('"hello" + 1')
    # With skip_type_check=True, the type error is not raised (but runtime error still occurs)
    with pytest.raises(MontyRuntimeError):
        tc_session.feed_run('"hello" + 1', skip_type_check=True)


def test_skip_type_check_does_not_accumulate(tc_session: MontySession):
    """skip_type_check=True bypasses the check AND keeps the snippet out of
    the type-check context — later feeds must not be checked against bindings
    the caller explicitly excluded from checking."""
    tc_session.feed_run('x = "hello"', skip_type_check=True)
    with pytest.raises(MontyTypingError) as exc_info:
        tc_session.feed_run('x + 1')
    assert str(exc_info.value) == snapshot("""\
error[unresolved-reference]: Name `x` used when not defined
 --> main.py:1:1
  |
1 | x + 1
  | ^
  |

""")


def test_type_check_runtime_error_does_not_pollute_state(tc_session: MontySession):
    """A snippet that fails at runtime must not leak definitions into later type checks."""
    with pytest.raises(MontyRuntimeError):
        tc_session.feed_run("""\
def foo(x: int) -> int:
    return x

1 / 0
""")
    with pytest.raises(MontyTypingError) as exc_info:
        tc_session.feed_run('foo("x")')
    assert str(exc_info.value) == snapshot("""\
error[unresolved-reference]: Name `foo` used when not defined
 --> main.py:1:1
  |
1 | foo("x")
  | ^^^
  |

""")
