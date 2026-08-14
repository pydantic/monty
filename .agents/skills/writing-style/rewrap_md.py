"""Rewrap markdown prose: one sentence per line, hard-wrapped at 120 chars.

Skips fenced code blocks, tables, headings, and frontmatter. Paragraph and
list-item lines are first joined, then split into sentences (one per line),
then any line over 120 chars is wrapped at word boundaries. List continuation
lines are indented to the content column of their marker.
"""

import re
import sys
import textwrap

WIDTH = 120

# Don't split after these (abbreviations, initialisms).
ABBREV = re.compile(r'\b(e\.g|i\.e|etc|vs|approx|cf|no|v[0-9.]*)\.$', re.IGNORECASE)
SENT_END = re.compile(r'([.!?][)"\']?)\s+(?=[A-Z`\[($~0-9])')
BULLET = re.compile(r'^(\s*)([-*+]|\d+\.)\s+')


def split_sentences(text: str) -> list[str]:
    parts: list[str] = []
    last = 0
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
        marker, indent = m.group(0), ' ' * len(m.group(0))
        text = ' '.join([buf[0][len(marker) :]] + [ln.strip() for ln in buf[1:]])
        for i, sent in enumerate(split_sentences(text)):
            out.extend(wrap(sent, marker if i == 0 else indent, indent))
    else:
        text = ' '.join(ln.strip() for ln in buf)
        for sent in split_sentences(text):
            out.extend(wrap(sent, '', ''))
    buf.clear()


def rewrap(src: str) -> str:
    out: list[str] = []
    buf: list[str] = []
    in_fence = in_front = False
    lines = src.splitlines()
    for i, line in enumerate(lines):
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
        if stripped.startswith('```'):
            flush(buf, out)
            in_fence = not in_fence
            out.append(line)
            continue
        if in_fence or stripped.startswith(('#', '|', '>')) or not stripped:
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
