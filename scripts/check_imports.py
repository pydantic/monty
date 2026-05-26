#!/usr/bin/env -S uv run --script
"""Check that `use` statements live at module scope, not inside function bodies.

Enforces the rule in CLAUDE.md that imports must live at the top of the file.
Top-level `use` statements and `use` statements inside `mod { ... }` blocks are
allowed; any `use` statement whose innermost enclosing item (walking outward)
is a `fn` is flagged as a local import.

Exits with code 1 if any local imports are found.

An individual local import can be explicitly permitted with a
`// allow local import` comment, placed either as a trailing comment on the
`use` line or on its own line immediately above:

    // allow local import
    use Opcode::*;

    use Opcode::*; // allow local import

Implementation sketch:
- A small Rust lexer masks strings (including raw strings), char literals,
  and line/block comments — replacing their contents with spaces so that
  `{`, `}`, and `use` tokens inside them don't confuse the scanner.
- The masked stream is walked character by character, maintaining a stack
  of block kinds classified from the text preceding each `{`. Each segment
  between braces (or semicolons) is tested for a `use` statement.
- When a `use` is found, the enclosing stack is walked from innermost out:
  hitting `fn` first means "local import" (flag it); hitting `mod` first
  means the `use` is at the top of a nested module (allowed).
"""

import re
import sys
from dataclasses import dataclass
from pathlib import Path

# Matches a `use` statement at the start of a trimmed segment. Handles visibility
# modifiers (`pub use`, `pub(crate) use`, `pub(super) use`, etc.).
USE_STMT_RE = re.compile(r'^\s*(pub\s*(\([^)]*\)\s*)?)?use\s')

# Keywords used to classify what kind of block a `{` opens. The classifier
# looks for whole-word matches in the text preceding the brace on the same
# "segment" (i.e. since the previous brace or start of line).
FN_RE = re.compile(r'\bfn\b')
MOD_RE = re.compile(r'\bmod\b')
TRAIT_RE = re.compile(r'\btrait\b')
IMPL_RE = re.compile(r'\bimpl\b')

# Escape hatch marker: a `// allow local import` comment exempts a specific
# `use` statement. The marker must appear inside an actual line comment (not
# inside a string) — the check runs against the comment text extracted during
# masking, so string contents that happen to include the phrase don't count.
ALLOW_MARKER = '// allow local import'

CHAR_LITERAL_RE = re.compile(
    r"'(?:"
    r'\\x[0-9a-fA-F]{2}'
    r'|\\u\{[0-9a-fA-F]+\}'
    r'|\\.'
    r"|[^'\\]"
    r")'"
)


@dataclass
class LexState:
    """Lexer state that persists across lines.

    `block_comment_depth` tracks nested `/* ... */` (Rust allows nesting).
    `raw_string_hashes` is the number of `#`s delimiting the current raw
    string, or `None` when not in one — raw strings can span many lines and
    only terminate at a `"` followed by exactly that many `#`s.
    """

    block_comment_depth: int = 0
    raw_string_hashes: int | None = None


def check_file(path: Path) -> list[str]:
    """Scan a Rust file and return `(path:line: stmt)` strings for each local import.

    Tracks the `// allow local import` escape hatch across lines: if the line
    preceding a `use` is a comment-only line carrying the marker, that `use`
    is exempted, and a trailing marker on the `use` line itself also exempts.
    """
    errors: list[str] = []
    stack: list[str] = []
    state = LexState()
    prev_line_allows_next = False
    with open(path, encoding='utf-8', errors='replace') as f:
        for line_num, line in enumerate(f, 1):
            masked, comment = mask_line(line, state)
            marker_here = ALLOW_MARKER in comment
            allow_this_line = marker_here or prev_line_allows_next
            scan_line(masked, stack, errors, path, line_num, allow=allow_this_line)
            # Only a standalone comment line carries the marker forward to the
            # following line. A trailing marker on a code line (e.g. next to a
            # `use` or a `let`) applies only to its own line.
            prev_line_allows_next = marker_here and masked.strip() == ''
    return errors


def main() -> int:
    """Walk ./crates for `.rs` files and report any local imports found."""
    all_errors: list[str] = []
    for rs_file in sorted(Path('./crates').rglob('*.rs')):
        all_errors += check_file(rs_file)
    if all_errors:
        print('Error: Found local `use` statements inside function bodies:', file=sys.stderr)
        print('Move these imports to the top of the file.', file=sys.stderr)
        print(file=sys.stderr)
        for error in all_errors:
            print(error, file=sys.stderr)
        return 1
    return 0


# ---------------------------------------------------------------------------
# Scanning
# ---------------------------------------------------------------------------


def scan_line(masked: str, stack: list[str], errors: list[str], path: Path, line_num: int, allow: bool) -> None:
    """Walk a masked line, updating `stack` and appending any flagged `use`s to `errors`.

    The line is split into segments at every `{` and `}`; within each segment,
    statements are split on `;` so that lines like `fn f() { use X; }` correctly
    attribute the `use` to the post-`{` stack (inside `fn`), not the pre-line
    stack (top-level). `allow=True` suppresses reporting for this entire line.
    """
    seg_start = 0
    for i, c in enumerate(masked):
        if c == '{':
            check_segment(masked[seg_start:i], stack, errors, path, line_num, allow)
            stack.append(classify(masked[seg_start:i]))
            seg_start = i + 1
        elif c == '}':
            check_segment(masked[seg_start:i], stack, errors, path, line_num, allow)
            if stack:
                stack.pop()
            seg_start = i + 1
    check_segment(masked[seg_start:], stack, errors, path, line_num, allow)


def check_segment(seg: str, stack: list[str], errors: list[str], path: Path, line_num: int, allow: bool) -> None:
    """Look for `use X;` statements in a between-braces segment and flag local ones."""
    if allow:
        return
    for stmt in seg.split(';'):
        if USE_STMT_RE.match(stmt) and should_flag(stack):
            errors.append(f'{path}:{line_num}: {stmt.strip()};')


def classify(prefix: str) -> str:
    """Classify a `{` block given the source text preceding it on its segment.

    Precedence (`fn` > `mod` > `trait` > `impl`) matters because a single
    prefix can contain several keywords — e.g. `impl Foo { fn bar() {` — and
    the classifier is called per-brace, so only the portion since the
    previous `{` / `}` appears here. Falls through to `'other'` for blocks
    that don't introduce a named item (match arms, closures, block
    expressions, unsafe blocks, struct/enum bodies, etc.).
    """
    if FN_RE.search(prefix):
        return 'fn'
    if MOD_RE.search(prefix):
        return 'mod'
    if TRAIT_RE.search(prefix):
        return 'trait'
    if IMPL_RE.search(prefix):
        return 'impl'
    return 'other'


def should_flag(stack: list[str]) -> bool:
    """Return True when the current position is inside a function body.

    Walks the stack outward from the innermost block. `fn` means we're inside
    a function (flag the import); `mod` means we've reached the surrounding
    module scope before any `fn` (allow the import). `impl`, `trait`, and
    `other` are transparent — we keep walking.
    """
    for kind in reversed(stack):
        if kind == 'fn':
            return True
        if kind == 'mod':
            return False
    return False


# ---------------------------------------------------------------------------
# Lexing: mask strings and comments so brace counting stays accurate
# ---------------------------------------------------------------------------


def mask_line(line: str, state: LexState) -> tuple[str, str]:
    """Replace string contents, char literals, and comments with spaces.

    Returns `(masked, line_comment)`. `masked` has the same length as the
    input so column positions are preserved; structural characters (`{`,
    `}`, `;`, identifier chars in code) are kept verbatim, and everything
    inside a string, char literal, or comment is turned into spaces.
    `line_comment` holds the raw text of the first `//` comment seen on
    this line (empty string if none), so callers can detect marker comments
    without having to rescan the line.
    """
    out: list[str] = []
    line_comment = ''
    i, n = 0, len(line)
    in_string = False
    while i < n:
        c = line[i]
        nxt = line[i + 1] if i + 1 < n else ''

        # Finish any block comment carried over from a previous line/region.
        if state.block_comment_depth > 0:
            if c == '*' and nxt == '/':
                state.block_comment_depth -= 1
                out.append('  ')
                i += 2
                continue
            if c == '/' and nxt == '*':
                state.block_comment_depth += 1
                out.append('  ')
                i += 2
                continue
            out.append(' ')
            i += 1
            continue

        # Finish any raw string carried across lines.
        if state.raw_string_hashes is not None:
            hashes = state.raw_string_hashes
            if c == '"' and line[i + 1 : i + 1 + hashes] == '#' * hashes:
                state.raw_string_hashes = None
                out.append(' ' * (1 + hashes))
                i += 1 + hashes
                continue
            out.append(' ')
            i += 1
            continue

        if in_string:
            if c == '\\' and i + 1 < n:
                out.append('  ')
                i += 2
                continue
            if c == '"':
                in_string = False
                out.append(' ')
                i += 1
                continue
            out.append(' ')
            i += 1
            continue

        # Line comment — blank rest of line, but remember its text.
        if c == '/' and nxt == '/':
            line_comment = line[i:].rstrip('\n')
            out.append(' ' * (n - i))
            break

        # Block comment opener.
        if c == '/' and nxt == '*':
            state.block_comment_depth += 1
            out.append('  ')
            i += 2
            continue

        # Raw string opener: `r`, optional `b`/`c` prefixes, N `#`s, then `"`.
        if is_raw_string_start(line, i):
            consumed, hashes, terminated = consume_raw_string(line, i)
            out.append(' ' * consumed)
            if not terminated:
                state.raw_string_hashes = hashes
            i += consumed
            continue

        # Byte/c-string prefixes followed by a normal string: skip the prefix
        # and fall through to the `"` branch by letting the next iteration
        # handle it. Prefixes themselves are identifier chars, not braces, so
        # leaving them unmasked is harmless for brace counting.
        if c == '"':
            in_string = True
            out.append(' ')
            i += 1
            continue

        # Char literal: `'x'`, `'\n'`, `'\x41'`, `'\u{0041}'`. Distinguish
        # from lifetimes (`'a`) by trying to match a full char literal.
        if c == "'":
            m = CHAR_LITERAL_RE.match(line, i)
            if m:
                out.append(' ' * (m.end() - i))
                i = m.end()
                continue
            out.append(c)
            i += 1
            continue

        out.append(c)
        i += 1

    return ''.join(out), line_comment


def is_raw_string_start(line: str, i: int) -> bool:
    """True if `line[i:]` begins a raw string literal (`r"`, `r#"`, `br"`, etc.)."""
    j = i
    if j < len(line) and line[j] in 'bc':
        j += 1
    if j < len(line) and line[j] == 'r':
        j += 1
    else:
        return False
    # At least one `#` or a `"` must follow for this to be a raw string.
    while j < len(line) and line[j] == '#':
        j += 1
    return j < len(line) and line[j] == '"'


def consume_raw_string(line: str, i: int) -> tuple[int, int, bool]:
    """Consume characters starting a raw string; return (chars_consumed, hash_count, terminated).

    If the closing delimiter is not on the same line, `terminated` is False
    and the caller should mark the lexer state as "inside a raw string with
    this many hashes" so subsequent lines continue masking correctly.
    """
    j = i
    if line[j] in 'bc':
        j += 1
    # Skip the `r`.
    j += 1
    hash_start = j
    while j < len(line) and line[j] == '#':
        j += 1
    hashes = j - hash_start
    # Skip the opening `"`.
    j += 1
    terminator = '"' + '#' * hashes
    end = line.find(terminator, j)
    if end == -1:
        return (len(line) - i, hashes, False)
    return (end + len(terminator) - i, hashes, True)


if __name__ == '__main__':
    sys.exit(main())
