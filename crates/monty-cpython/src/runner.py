import ast as _ast
import asyncio as _asyncio
import typing as _typing

# inspect.CO_COROUTINE — set on a code object the compiler turned into a coroutine
# because it contains a top-level `await`. A bare expression that merely *evaluates*
# to a coroutine (e.g. calling an `async def`) does NOT get this flag, so we only
# auto-run genuine top-level-await snippets, never arbitrary coroutine values.
CO_COROUTINE = 0x80
# Allow `await`/`async for`/`async with` at module level (the flag the asyncio
# REPL and IPython use); the compiled unit then needs driving to completion.
TOP_LEVEL_AWAIT = _ast.PyCF_ALLOW_TOP_LEVEL_AWAIT


def run(code: str, ns: dict[str, _typing.Any], script_name: str) -> _typing.Any:
    """Execute `code` REPL-style: a trailing expression becomes the value.

    Mirrors how IPython/the stdlib REPL split a cell — run the body in `exec`
    mode, then evaluate a trailing *expression* statement separately so its value
    can be returned. The split node keeps its original location, so a traceback
    from the trailing expression still points at the right line.

    `script_name` is the filename the code is compiled under (the session's
    `Configure.script_name`), so CPython tracebacks and `SyntaxError`s report it
    rather than an internal placeholder. It is also how the Rust side tells user
    frames apart from this module's driver frames when rebuilding the traceback.

    Top-level `await` is supported: both halves are compiled with
    `PyCF_ALLOW_TOP_LEVEL_AWAIT`. If *either* half is a coroutine, both are driven
    in a single `asyncio.run` event loop (see `drive_async`) so async objects the
    body creates keep their loop affinity in the trailing expression. Purely
    synchronous snippets never touch asyncio.
    """
    module = _ast.parse(code, script_name, 'exec')
    trailing_expr = None
    if module.body and isinstance(module.body[-1], _ast.Expr):
        trailing_expr = _typing.cast(_ast.Expr, module.body.pop()).value
    body_code = compile(module, script_name, 'exec', flags=TOP_LEVEL_AWAIT)
    expr_code = (
        None
        if trailing_expr is None
        else compile(_ast.Expression(trailing_expr), script_name, 'eval', flags=TOP_LEVEL_AWAIT)
    )
    body_async = bool(body_code.co_flags & CO_COROUTINE)
    expr_async = expr_code is not None and bool(expr_code.co_flags & CO_COROUTINE)
    if body_async or expr_async:
        # One loop for the whole cell. Splitting body and trailing expression
        # across two `asyncio.run` calls would give each its own loop, so an
        # object created in the body (a `Lock`, `Queue`, task, future, ...) would
        # be bound to a loop already closed by the time the expression awaits it.
        return _asyncio.run(drive_async(body_code, body_async, expr_code, expr_async, ns))
    else:
        eval(body_code, ns)
        return None if expr_code is None else eval(expr_code, ns)


async def drive_async(
    body_code: _typing.Any,
    body_async: bool,
    expr_code: _typing.Any,
    expr_async: bool,
    ns: dict[str, _typing.Any],
) -> _typing.Any:
    """Run a cell's body then its trailing expression in one event loop.

    Either half may be a top-level-await coroutine (`*_async`); the other is a
    plain `eval`. Driving both on the same loop preserves loop affinity for any
    async object the body hands to the trailing expression. Returns the trailing
    expression's value (or `None` when there is none).
    """
    result = eval(body_code, ns)
    if body_async:
        await result
    if expr_code is None:
        return None
    result = eval(expr_code, ns)
    return (await result) if expr_async else result
