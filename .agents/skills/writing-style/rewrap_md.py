"""Rewrap markdown prose: one sentence per line, hard-wrapped at 120 chars.

Skips fenced code blocks (``` or ~~~, closed only by a matching fence),
tables, headings, mkdocs admonition markers (`!!!`/`???`), thematic breaks /
setext underlines, and frontmatter.
Paragraph and list-item lines are joined, split into sentences (one per
line), then any line over 120 chars is wrapped at word boundaries. List
continuation lines are indented to the content column of their marker;
indented paragraphs keep their indent. Hard breaks (two trailing spaces)
are preserved.

Known limitations: tables must use leading pipes (house style does), and a
sentence starting with a lowercase letter stays on the previous line — the
splitter requires a capital/code start so abbreviations like "e.g." never
split.
"""

import re
import sys
import textwrap

WIDTH = 120

# Don't split after these (abbreviations, initialisms).
ABBREV = re.compile(r'\b(e\.g|i\.e|etc|vs|approx|cf|no|v[0-9.]*)\.$', re.IGNORECASE)
SENT_END = re.compile(r'([.!?][)"\']?)\s+(?=[A-Z`\[($~0-9])')
BULLET = re.compile(r'^(\s*)([-*+]|\d+\.)\s+')
FENCE = re.compile(r'^(`{3,}|~{3,})')
# Thematic breaks and setext underlines: a run of -/=/*/_ (spaces allowed).
BREAK_LINE = re.compile(r'^(?:[-=*_]\s*){3,}$|^={1,2}$|^-{1,2}$')


def split_sentences(text: str) -> list[str]:
    parts, last = [], 0
    for m in SENT_END.finditer(text):
        candidate = text[last : m.end(1)]
        if ABBREV.search(candidate):
            continue
        parts.append(candidate.strip())
        last = m.end()
    parts.append(text[last:].strip())
    return [p for p in parts if p]


def wrap(sentence: str, first_prefix: str, cont_prefix: str) -> list[str]:
    return textwrap.wrap(
        sentence,
        width=WIDTH,
        initial_indent=first_prefix,
        subsequent_indent=cont_prefix,
        break_long_words=False,
        break_on_hyphens=False,
    ) or [first_prefix.rstrip()]


def flush(buf: list[str], out: list[str]) -> None:
    if not buf:
        return
    m = BULLET.match(buf[0])
    if m:
        marker = m.group(0)
        first_prefix, cont_prefix = marker, ' ' * len(marker)
        texts = [buf[0][len(marker) :]] + [ln.strip() for ln in buf[1:]]
    else:
        # A plain block keeps its indent, so the continuation paragraph of a
        # list item stays indented under its marker.
        indent = buf[0][: len(buf[0]) - len(buf[0].lstrip())]
        first_prefix = cont_prefix = indent
        texts = [ln.strip() for ln in buf]
    first = True

    def emit(segment: list[str], hard_break: bool) -> None:
        nonlocal first
        for sent in split_sentences(' '.join(segment)):
            out.extend(wrap(sent, first_prefix if first else cont_prefix, cont_prefix))
            first = False
        if hard_break and out:
            out[-1] += '  '

    # A line ending in two spaces is a markdown hard break: wrap each side
    # separately and keep the marker on the break's last line.
    segment: list[str] = []
    for raw, text in zip(buf, texts):
        segment.append(text)
        if raw.endswith('  '):
            emit(segment, True)
            segment = []
    if segment:
        emit(segment, False)
    buf.clear()


def rewrap(src: str) -> str:
    out: list[str] = []
    buf: list[str] = []
    fence: str | None = None  # the opening run, e.g. '```' or '~~~~'
    in_front = False
    for i, line in enumerate(src.splitlines()):
        stripped = line.strip()
        if i == 0 and stripped == '---':
            in_front = True
            out.append(line)
            continue
        if in_front:
            out.append(line)
            if stripped == '---':
                in_front = False
            continue
        if fence:
            out.append(line)
            # A closing fence is a run of the opening character at least as
            # long as the opener, and nothing else on the line.
            if len(stripped) >= len(fence) and set(stripped) == {fence[0]}:
                fence = None
            continue
        if opening := FENCE.match(stripped):
            flush(buf, out)
            fence = opening.group(1)
            out.append(line)
            continue
        # Setext underlines are indistinguishable from thematic breaks here;
        # both survive because the buffered heading text flushes unchanged
        # (short, single line) and the marker line passes through verbatim.
        # `!!!`/`???` are mkdocs admonition markers; their indented body is a
        # separate block, so the marker line must not join it.
        if not stripped or stripped.startswith(('#', '|', '>', '!!!', '???')) or BREAK_LINE.match(stripped):
            flush(buf, out)
            out.append(line)
            continue
        # A new bullet starts its own block; continuation lines join the current one.
        if BULLET.match(line):
            flush(buf, out)
        buf.append(line)
    flush(buf, out)
    return '\n'.join(out) + '\n'


for path in sys.argv[1:]:
    with open(path) as f:
        original = f.read()
    result = rewrap(original)
    with open(path, 'w') as f:
        f.write(result)
    print(f'{path}: {len(original.splitlines())} -> {len(result.splitlines())} lines')
