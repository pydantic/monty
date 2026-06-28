import ast as _ast
import asyncio as _asyncio
import builtins as _builtins
import typing as _typing

# inspect.CO_COROUTINE — set on a code object the compiler turned into a coroutine
# because it contains a top-level `await`. A bare expression that merely *evaluates*
# to a coroutine (e.g. calling an `async def`) does NOT get this flag, so we only
# auto-run genuine top-level-await snippets, never arbitrary coroutine values.
_CO_COROUTINE = 0x80
# Allow `await`/`async for`/`async with` at module level (the flag the asyncio
# REPL and IPython use); the compiled unit then needs driving to completion.
_TOP_LEVEL_AWAIT = _ast.PyCF_ALLOW_TOP_LEVEL_AWAIT

if _typing.TYPE_CHECKING:

    class _HostBridge(_typing.Protocol):
        # Eagerly resolves `name` via a NameLookup round-trip: returns the host
        # value, or an ExternalFunction proxy for a host function, or raises
        # NameError if the parent reports the name as undefined. Caching of
        # function proxies lives in the host, so `__missing__` stays a passthrough.
        def get(self, name: str) -> _typing.Any: ...


class _CallbackGlobals(dict[str, _typing.Any]):  # pyright: ignore[ reportUnusedClass]
    """Execution globals whose missing-name lookups resolve through the host.

    Because this is a `dict` *subclass*, CPython resolves unbound global names
    through `__missing__`. Builtins and dunders fall through (raise `KeyError`);
    any other unbound name is resolved by the host — to a value, an external
    function proxy, or a `NameError` — all decided in Rust by `HostBridge.get`.
    """

    def __init__(self, host: _HostBridge):
        super().__init__()
        self._host = host

    def __missing__(self, name: str) -> _typing.Any:
        if name.startswith('__') or hasattr(_builtins, name):
            raise KeyError(name)
        else:
            return self._host.get(name)


def _run(code: str, ns: dict[str, _typing.Any]) -> _typing.Any:  # pyright: ignore[reportUnusedFunction]
    """Execute `code` REPL-style: a trailing expression becomes the value.

    Mirrors how IPython/the stdlib REPL split a cell — run the body in `exec`
    mode, then evaluate a trailing *expression* statement separately so its value
    can be returned. The split node keeps its original location, so a traceback
    from the trailing expression still points at the right line.

    Top-level `await` is supported: both halves are compiled with
    `PyCF_ALLOW_TOP_LEVEL_AWAIT`. If *either* half is a coroutine, both are driven
    in a single `asyncio.run` event loop (see `_drive_async`) so async objects the
    body creates keep their loop affinity in the trailing expression. Purely
    synchronous snippets never touch asyncio.
    """
    module = _ast.parse(code, '<sandbox>', 'exec')
    trailing_expr = None
    if module.body and isinstance(module.body[-1], _ast.Expr):
        trailing_expr = _typing.cast(_ast.Expr, module.body.pop()).value
    body_code = compile(module, '<sandbox>', 'exec', flags=_TOP_LEVEL_AWAIT)
    expr_code = (
        None
        if trailing_expr is None
        else compile(_ast.Expression(trailing_expr), '<sandbox>', 'eval', flags=_TOP_LEVEL_AWAIT)
    )
    body_async = bool(body_code.co_flags & _CO_COROUTINE)
    expr_async = expr_code is not None and bool(expr_code.co_flags & _CO_COROUTINE)
    if body_async or expr_async:
        # One loop for the whole cell. Splitting body and trailing expression
        # across two `asyncio.run` calls would give each its own loop, so an
        # object created in the body (a `Lock`, `Queue`, task, future, ...) would
        # be bound to a loop already closed by the time the expression awaits it.
        return _asyncio.run(_drive_async(body_code, body_async, expr_code, expr_async, ns))
    else:
        eval(body_code, ns)
        return None if expr_code is None else eval(expr_code, ns)


async def _drive_async(
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
