import ast
import asyncio
import linecache
from typing import Any, cast

# inspect.CO_COROUTINE — set on a code object the compiler turned into a coroutine
# because it contains a top-level `await`. A bare expression that merely *evaluates*
# to a coroutine (e.g. calling an `async def`) does NOT get this flag, so we only
# auto-run genuine top-level-await snippets, never arbitrary coroutine values.
CO_COROUTINE = 0x80
# Allow `await`/`async for`/`async with` at module level (the flag the asyncio
# REPL and IPython use); the compiled unit then needs driving to completion.
TOP_LEVEL_AWAIT = ast.PyCF_ALLOW_TOP_LEVEL_AWAIT

# Monotonic counter and registry for the per-feed filenames fed code compiles
# under. Each feed gets a unique `<input-N>` name so a traceback can resolve the
# right source even for a frame from a function defined in an *earlier* feed —
# every feed shares one session, but their line numbers would otherwise collide.
# Mirrors monty's own REPL (`MontyRepl.sources` keyed by `<python-input-N>`).
# Process-global is fine: a worker serves exactly one session.
input_counter = 0
input_files: set[str] = set()


def run(code: str, ns: dict[str, Any], script_name: str) -> Any:
    """Execute `code` REPL-style: a trailing expression becomes the value.

    Like IPython/the stdlib REPL, the body runs in `exec` mode and a trailing
    expression is evaluated separately so its value can be returned; the split
    node keeps its location so tracebacks still point at the right line.

    Each snippet compiles under a unique `<input-N>` filename, registered in
    `linecache` so `extract_traceback` can recover source lines — even for frames
    from functions defined in earlier feeds. Runtime frames get `script_name`
    substituted there; syntax errors have no user frame, so they are rewritten here.

    Top-level `await` is supported: if either half is a coroutine, both run in one
    `asyncio.run` loop (see `drive_async`) to keep async objects' loop affinity.
    Purely synchronous snippets never touch asyncio.
    """
    global input_counter
    filename = f'<input-{input_counter}>'
    input_counter += 1
    input_files.add(filename)
    # `mtime=None` marks a non-file cache entry that `linecache.checkcache`
    # leaves in place (it would otherwise try to `stat` the fake filename).
    linecache.cache[filename] = (len(code), None, code.splitlines(keepends=True), filename)
    try:
        module = ast.parse(code, filename, 'exec')
        trailing_expr = None
        if module.body and isinstance(module.body[-1], ast.Expr):
            trailing_expr = cast(ast.Expr, module.body.pop()).value
        body_code = compile(module, filename, 'exec', flags=TOP_LEVEL_AWAIT)
        expr_code = (
            None
            if trailing_expr is None
            else compile(ast.Expression(trailing_expr), filename, 'eval', flags=TOP_LEVEL_AWAIT)
        )
    except SyntaxError as exc:
        exc.filename = script_name
        if len(exc.args) >= 2 and isinstance(exc.args[1], tuple) and exc.args[1]:
            location = cast(tuple[Any, ...], exc.args[1])
            exc.args = (exc.args[0], (script_name, *location[1:]), *exc.args[2:])
        raise
    body_async = bool(body_code.co_flags & CO_COROUTINE)
    expr_async = expr_code is not None and bool(expr_code.co_flags & CO_COROUTINE)
    if body_async or expr_async:
        # One loop for the whole cell. Splitting body and trailing expression
        # across two `asyncio.run` calls would give each its own loop, so an
        # object created in the body (a `Lock`, `Queue`, task, future, ...) would
        # be bound to a loop already closed by the time the expression awaits it.
        return asyncio.run(drive_async(body_code, body_async, expr_code, expr_async, ns))
    else:
        eval(body_code, ns)
        return None if expr_code is None else eval(expr_code, ns)


async def drive_async(
    body_code: Any,
    body_async: bool,
    expr_code: Any,
    expr_async: bool,
    ns: dict[str, Any],
) -> Any:
    """Run a cell's body then its trailing expression in one event loop.

    Either half may be a top-level-await coroutine (`*_async`); driving both on
    the same loop preserves loop affinity for any async object the body hands to
    the expression. Returns the expression's value (or `None`).
    """
    result = eval(body_code, ns)
    if body_async:
        await result
    if expr_code is None:
        return None
    result = eval(expr_code, ns)
    return (await result) if expr_async else result


# One structured frame, shaped to map directly onto monty's `StackFrame`:
# (filename, start_line, start_col, end_line, end_col, frame_name, preview_line,
#  hide_caret, hide_frame_name). Lines and columns are 1-based; columns count
# characters (not bytes). `frame_name` is `None` for module-level code.
Frame = tuple[str, int, int, int, int, str | None, str | None, bool, bool]


def extract_traceback(tb: Any, script_name: str) -> list[Frame]:
    """Rebuild the sandbox traceback as `Frame` tuples for the Rust side.

    Keeps only frames from fed code (the `<input-N>` files registered in
    `linecache`), dropping the runner's driver frames, and reports each under
    `script_name`, outermost first. Best-effort: any failure (e.g. a sandbox that
    monkey-patched `traceback`) yields an empty list rather than masking the
    original exception, whose type and message are carried separately.
    """
    try:
        return _extract_traceback(tb, script_name)
    except Exception:
        return []


def _extract_traceback(tb: Any, script_name: str) -> list[Frame]:
    """Walk `tb` into `Frame` tuples with source previews and caret spans.

    `extract_tb` is imported at call time so a sandbox that monkey-patched
    `traceback` makes this raise (handled by `extract_traceback`) rather than
    silently driving off a patched module.
    """
    from traceback import extract_tb

    frames: list[Frame] = []
    for fs in extract_tb(tb):
        if fs.filename not in input_files:
            continue
        # The *unstripped* source line (minus its line terminator): monty's
        # `StackFrame` stores the full line and trims it at render time, and
        # CPython's byte columns index into it. Strip `\r` as well as `\n` so a
        # CRLF-fed snippet does not leave a stray carriage return in the preview.
        lineno = cast(int, fs.lineno)
        line = linecache.getline(fs.filename, lineno)
        preview = line.rstrip('\r\n') if line else None
        frame_name = None if fs.name == '<module>' else fs.name
        start_col = 0
        end_col = 0
        hide_caret = True
        # Carets need a same-line span and a preview to render against.
        if preview is not None and fs.colno is not None and fs.end_colno is not None and fs.end_lineno == lineno:
            # CPython reports columns as UTF-8 byte offsets; monty wants 1-based
            # character columns.
            start_col = byte_to_char(preview, fs.colno) + 1
            end_col = byte_to_char(preview, fs.end_colno) + 1
            hide_caret = is_raise_statement(preview)
        frames.append((script_name, lineno, start_col, lineno, end_col, frame_name, preview, hide_caret, False))
    return frames


def is_raise_statement(preview: str) -> bool:
    """Whether `preview`'s first token is `raise`, the caret-visibility heuristic.

    CPython hides carets for `raise` and shows them otherwise; we mirror only that
    case. Its other no-caret cases (attribute access, bare names, full-line
    `x = f()` calls) over-draw a whole-line underline — cosmetic, and it keeps us
    off `traceback` internals.
    """
    head = preview.split(maxsplit=1)
    return bool(head) and head[0] == 'raise'


def byte_to_char(line: str, byte_offset: int) -> int:
    """Convert a 0-based UTF-8 byte offset into `line` to a 0-based char offset."""
    return len(line.encode('utf-8')[:byte_offset].decode('utf-8', 'replace'))
