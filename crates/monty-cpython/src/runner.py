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

# Characters that make up a caret-marker line in CPython's rendered traceback
# (spaces plus the primary `~` / secondary `^` anchors). Used to detect whether
# CPython chose to draw carets for a frame — see `extract_traceback`.
_CARET_CHARS = frozenset(' ~^')

# Monotonic counter and registry for the per-feed filenames fed code compiles
# under. Each feed gets a unique `<input-N>` name so a traceback can resolve the
# right source even for a frame from a function defined in an *earlier* feed —
# every feed shares one session, but their line numbers would otherwise collide.
# Mirrors monty's own REPL (`MontyRepl.sources` keyed by `<python-input-N>`).
# Process-global is fine: a worker serves exactly one session.
_input_counter = 0
_input_files: set[str] = set()


def run(code: str, ns: dict[str, Any]) -> Any:
    """Execute `code` REPL-style: a trailing expression becomes the value.

    Mirrors how IPython/the stdlib REPL split a cell — run the body in `exec`
    mode, then evaluate a trailing *expression* statement separately so its value
    can be returned. The split node keeps its original location, so a traceback
    from the trailing expression still points at the right line.

    The snippet is compiled under a unique internal `<input-N>` filename and its
    source is registered in `linecache`, so a later `extract_traceback` can
    recover each frame's source line (and CPython's caret anchors) — even for
    frames from functions defined in earlier feeds. The parent-visible filename
    is substituted in `extract_traceback`, not here.

    Top-level `await` is supported: both halves are compiled with
    `PyCF_ALLOW_TOP_LEVEL_AWAIT`. If *either* half is a coroutine, both are driven
    in a single `asyncio.run` event loop (see `drive_async`) so async objects the
    body creates keep their loop affinity in the trailing expression. Purely
    synchronous snippets never touch asyncio.
    """
    global _input_counter
    filename = f'<input-{_input_counter}>'
    _input_counter += 1
    _input_files.add(filename)
    # `mtime=None` marks a non-file cache entry that `linecache.checkcache`
    # leaves in place (it would otherwise try to `stat` the fake filename).
    linecache.cache[filename] = (len(code), None, code.splitlines(keepends=True), filename)
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


# One structured frame, shaped to map directly onto monty's `StackFrame`:
# (filename, start_line, start_col, end_line, end_col, frame_name, preview_line,
#  hide_caret, hide_frame_name). Lines and columns are 1-based; columns count
# characters (not bytes). `frame_name` is `None` for module-level code.
Frame = tuple[str, int, int, int, int, str | None, str | None, bool, bool]


def extract_traceback(tb: Any, script_name: str) -> list[Frame]:
    """Rebuild the sandbox traceback as structured frames for the Rust side.

    Walks `tb`, keeping only frames from fed code (the `<input-N>` files this
    module registered in `linecache`) so the runner's own driver frames are
    dropped. Returns one `Frame` tuple per frame, outermost first.

    `script_name` is reported as every frame's filename, so a multi-feed session
    shows the session's name rather than the internal per-feed keys. Whether to
    draw caret markers — and the exact anchored span — is taken from CPython's
    own machinery, so `raise`/whole-line cases match CPython (and monty) exactly.
    """
    from traceback import StackSummary, extract_tb

    frames: list[Frame] = []
    for fs in extract_tb(tb):
        if fs.filename not in _input_files:
            continue
        # The *unstripped* source line: monty's `StackFrame` stores the full line
        # and trims it at render time, and CPython's byte columns index into it.
        lineno = cast(int, fs.lineno)
        line = linecache.getline(fs.filename, lineno)
        preview = line.rstrip('\n') if line else None
        frame_name = None if fs.name == '<module>' else fs.name
        start_col = 0
        end_col = 0
        hide_caret = True
        # Carets need a same-line anchored span and a preview to render against.
        if preview is not None and fs.colno is not None and fs.end_colno is not None and fs.end_lineno == lineno:
            # CPython reports columns as UTF-8 byte offsets; monty wants 1-based
            # character columns.
            start_col = byte_to_char(preview, fs.colno) + 1
            end_col = byte_to_char(preview, fs.end_colno) + 1
            # Defer the show/hide decision to CPython so `raise` (no caret) and
            # whole-line calls/binops (caret) match its renderer. The caret is
            # hidden when CPython renders *no* marker line for the frame.
            rendered = ''.join(StackSummary.from_list([fs]).format())
            hide_caret = not any(
                set(part) <= _CARET_CHARS and ('~' in part or '^' in part) for part in rendered.splitlines()
            )
        frames.append((script_name, lineno, start_col, lineno, end_col, frame_name, preview, hide_caret, False))
    return frames


def byte_to_char(line: str, byte_offset: int) -> int:
    """Convert a 0-based UTF-8 byte offset into `line` to a 0-based char offset."""
    return len(line.encode('utf-8')[:byte_offset].decode('utf-8', 'replace'))
