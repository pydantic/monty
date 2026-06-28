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
    `PyCF_ALLOW_TOP_LEVEL_AWAIT`, and any half that the compiler turned into a
    coroutine is driven to completion with `asyncio.run`. Purely synchronous
    snippets never touch asyncio.
    """
    module = _ast.parse(code, '<sandbox>', 'exec')
    trailing_expr = None
    if module.body and isinstance(module.body[-1], _ast.Expr):
        trailing_expr = _typing.cast(_ast.Expr, module.body.pop()).value
    _drive(compile(module, '<sandbox>', 'exec', flags=_TOP_LEVEL_AWAIT), ns)
    if trailing_expr is None:
        return None
    else:
        return _drive(compile(_ast.Expression(trailing_expr), '<sandbox>', 'eval', flags=_TOP_LEVEL_AWAIT), ns)


def _drive(code: _typing.Any, ns: dict[str, _typing.Any]) -> _typing.Any:
    """Run a compiled code object, awaiting it if it's a top-level coroutine."""
    result = eval(code, ns)
    return _asyncio.run(result) if code.co_flags & _CO_COROUTINE else result
