"""Extract model prices from a 100 KB pricing page through a BeautifulSoup-style proxy.

The page never enters the model's context: `beautiful_soup` returns a `Tag` host
object and `find`, `find_all`, `select` and `get_text` run on the host (the February
2026 demo's `bs.py`). Each model is recorded through `record_model_info`, which
validates with pydantic, so a wrong price shape is reported rather than stored.

The prompt also carries last run's "optimal code", which used a class name the page
has since renamed: reusing it blindly finds nothing.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from html.parser import HTMLParser
from typing import Any

from pydantic import BaseModel, ValidationError

from evals.harness.evaluators import Predicate
from evals.harness.task import Task
from pydantic_monty import ClassInstance

MODELS = [
    ('nimbus-4', 'Nimbus 4', 'Flagship reasoning model', 12.0, 60.0, 200_000),
    ('nimbus-4-mini', 'Nimbus 4 Mini', 'Fast and cheap', 0.8, 3.2, 128_000),
    ('cirrus-2', 'Cirrus 2', 'Balanced general model', 3.0, 15.0, 200_000),
    ('cirrus-2-lite', 'Cirrus 2 Lite', 'Latency optimised', 0.4, 1.6, 64_000),
    ('stratus-embed', 'Stratus Embed', 'Embeddings', 0.1, 0.0, 8_000),
    ('nimbus-3', 'Nimbus 3 (deprecated)', 'Previous flagship', 15.0, 75.0, 100_000),
]
EXPECTED_MODELS = {
    unique_id: {
        'unique_id': unique_id,
        'name': name,
        'description': description,
        'input_mtok': input_mtok,
        'output_mtok': output_mtok,
        'attributes': {'context_window': context},
    }
    for unique_id, name, description, input_mtok, output_mtok, context in MODELS
    if 'deprecated' not in name
}


def _page() -> str:
    """Pricing table inside enough navigation and marketing boilerplate to reach 100 KB."""
    rows = '\n'.join(
        f'<tr data-model="{uid}"><td class="model"><strong>{name}</strong><br><span class="desc">{desc}</span></td>'
        f'<td class="price input">${inp:.2f} / MTok</td><td class="price output">${out:.2f} / MTok</td>'
        f'<td class="context">{ctx:,} tokens</td></tr>'
        for uid, name, desc, inp, out, ctx in MODELS
    )
    filler = '<p class="marketing">Build with confidence on infrastructure trusted by teams everywhere.</p>\n'
    nav = ''.join(f'<li><a href="/docs/page-{i}">Documentation page {i}</a></li>\n' for i in range(400))
    return (
        '<!doctype html><html><head><title>Pricing</title></head><body>\n'
        f'<nav><ul>{nav}</ul></nav>\n'
        f'{filler * 600}'
        '<main><h1>Pricing</h1>\n'
        '<table id="models" class="pricing"><thead><tr><th>Model</th><th>Input</th><th>Output</th><th>Context</th></tr></thead>\n'
        f'<tbody>\n{rows}\n</tbody></table>\n'
        f'{filler * 400}'
        '</main><footer><p>Prices in USD per million tokens.</p></footer></body></html>\n'
    )


HTML = _page()

PREVIOUS_CODE = """
page = beautiful_soup(html)
for row in select(page, 'table.price-table tbody tr'):
    cells = find_all(row, 'td')
    name = get_text(find(cells[0], 'strong'), strip=True)
    record_model_info({
        'unique_id': get(row, 'data-model'),
        'name': name,
        'description': get_text(find(cells[0], 'span'), strip=True),
        'input_mtok': float(get_text(cells[1]).split('$')[1].split(' ')[0]),
        'output_mtok': float(get_text(cells[2]).split('$')[1].split(' ')[0]),
        'attributes': {'context_window': int(get_text(cells[3]).split(' ')[0].replace(',', ''))},
    })
"""


@dataclass
class Node:
    """One parsed element; text children are plain strings."""

    name: str
    attrs: dict[str, str] = field(default_factory=dict)
    children: list[Node | str] = field(default_factory=list)

    def text(self) -> str:
        return ''.join(c if isinstance(c, str) else c.text() for c in self.children)

    def html(self) -> str:
        attrs = ''.join(f' {k}="{v}"' for k, v in self.attrs.items())
        inner = ''.join(c if isinstance(c, str) else c.html() for c in self.children)
        if self.name == '[document]':
            return inner
        return f'<{self.name}{attrs}>{inner}</{self.name}>'

    def walk(self) -> list[Node]:
        out: list[Node] = []
        for child in self.children:
            if isinstance(child, Node):
                out.append(child)
                out.extend(child.walk())
        return out


_VOID = {'br', 'meta', 'link', 'img', 'hr', 'input'}


class _Builder(HTMLParser):
    """Builds a `Node` tree; void elements never open a scope."""

    def __init__(self) -> None:
        super().__init__()
        self.root = Node('[document]')
        self.stack = [self.root]

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        node = Node(tag, {k: v or '' for k, v in attrs})
        self.stack[-1].children.append(node)
        if tag not in _VOID:
            self.stack.append(node)

    def handle_endtag(self, tag: str) -> None:
        for i in range(len(self.stack) - 1, 0, -1):
            if self.stack[i].name == tag:
                del self.stack[i:]
                return

    def handle_data(self, data: str) -> None:
        self.stack[-1].children.append(data)


def _parse(html: str) -> Node:
    builder = _Builder()
    builder.feed(html)
    return builder.root


@dataclass
class Tag:
    """What the sandbox sees: a mirror of a BeautifulSoup `Tag`, carrying its own HTML."""

    name: str
    attrs: dict[str, str]
    string: str | None
    text: str
    html: str


def _tag(node: Node) -> ClassInstance:
    text = node.text()
    only_text = len(node.children) == 1 and isinstance(node.children[0], str)
    return ClassInstance(
        Tag(node.name, dict(node.attrs), text if only_text else None, text, node.html()), eager_attrs='all'
    )


def _root(tag: Tag) -> Node:
    """Re-parse the tag's HTML; a single element is unwrapped so searches stay inside it."""
    doc = _parse(tag.html)
    elements = [c for c in doc.children if isinstance(c, Node)]
    return (
        elements[0] if len(elements) == 1 and not any(isinstance(c, str) and c.strip() for c in doc.children) else doc
    )


def _matches(node: Node, name: str | None, attrs: dict[str, str] | None) -> bool:
    if name is not None and node.name != name:
        return False
    for key, value in (attrs or {}).items():
        actual = node.attrs.get(key)
        if actual is None:
            return False
        if key == 'class':
            if value not in actual.split():
                return False
        elif actual != value:
            return False
    return True


def _simple_selector(node: Node, selector: str) -> bool:
    match = re.fullmatch(r'([a-zA-Z][\w-]*)?(#[\w-]+)?((?:\.[\w-]+)*)', selector)
    if match is None:
        raise ValueError(f'unsupported selector {selector!r}')
    name, id_part, classes = match.group(1), match.group(2), match.group(3)
    if name and node.name != name:
        return False
    if id_part and node.attrs.get('id') != id_part[1:]:
        return False
    have = set(node.attrs.get('class', '').split())
    return all(c in have for c in classes.split('.') if c)


def _select_nodes(root: Node, selector: str) -> list[Node]:
    parts = selector.split()
    candidates = [root]
    for part in parts:
        found: list[Node] = []
        for candidate in candidates:
            for node in candidate.walk():
                if _simple_selector(node, part) and node not in found:
                    found.append(node)
        candidates = found
    return candidates


def beautiful_soup(html: str) -> ClassInstance:
    """Host function: parse HTML and return the document `Tag`."""
    return _tag(_parse(html))


def find(tag: Tag, name: str | None = None, attrs: dict[str, str] | None = None) -> ClassInstance | None:
    """Host function: first descendant matching `name` and `attrs`."""
    for node in _root(tag).walk():
        if _matches(node, name, attrs):
            return _tag(node)
    return None


def find_all(
    tag: Tag, name: str | None = None, attrs: dict[str, str] | None = None, limit: int | None = None
) -> list[ClassInstance]:
    """Host function: every descendant matching `name` and `attrs`."""
    found = [_tag(node) for node in _root(tag).walk() if _matches(node, name, attrs)]
    return found[:limit] if limit else found


def select(tag: Tag, selector: str) -> list[ClassInstance]:
    """Host function: descendants matching a CSS selector of tag, `.class`, `#id` and descendant parts."""
    return [_tag(node) for node in _select_nodes(_root(tag), selector)]


def select_one(tag: Tag, selector: str) -> ClassInstance | None:
    """Host function: the first `select` match."""
    found = select(tag, selector)
    return found[0] if found else None


def get(tag: Tag, key: str, default: str | None = None) -> str | None:
    """Host function: an attribute value."""
    return tag.attrs.get(key, default)


def get_text(tag: Tag, separator: str = '', strip: bool = False) -> str:
    """Host function: the text inside the tag."""
    pieces = [c for c in _root(tag).text().split('\n')] if separator else [_root(tag).text()]
    text = separator.join(p.strip() if strip else p for p in pieces)
    return text.strip() if strip else text


class ModelInfo(BaseModel):
    """The record `record_model_info` accepts."""

    unique_id: str
    name: str
    description: str | None = None
    input_mtok: float
    output_mtok: float
    attributes: dict[str, float | int | str] | None = None


RECORDED: dict[str, dict[str, Any]] = {}


def record_model_info(model_information: dict[str, Any]) -> str:
    """Host function: validate and store one model's pricing record."""
    try:
        info = ModelInfo.model_validate(model_information)
    except ValidationError as exc:
        return f'Invalid model information: {exc.error_count()} error(s): {exc.errors()[0]["msg"]}'
    RECORDED[info.unique_id] = info.model_dump()
    return f'Model information recorded successfully for {info.unique_id}'


def _recorded_matches(_result: object) -> bool:
    """Every non-deprecated model recorded exactly, and nothing else."""
    return RECORDED == EXPECTED_MODELS


STUBS = '''
from dataclasses import dataclass
from typing import Any

@dataclass
class Tag:
    """A parsed HTML element."""
    name: str
    attrs: dict[str, str]
    string: str | None
    text: str
    html: str

html: str
"""The raw HTML of the pricing page (about 100 KB)."""

previous_code: str
"""The code that extracted this page last time; the page may have changed since."""

def beautiful_soup(html: str) -> Tag:
    """Parse HTML and return the document as a `Tag`."""
    ...

def find(tag: Tag, name: str | None = None, attrs: dict[str, str] | None = None) -> Tag | None:
    """First descendant with this tag name and attributes (`class` matches any class)."""
    ...

def find_all(tag: Tag, name: str | None = None, attrs: dict[str, str] | None = None, limit: int | None = None) -> list[Tag]:
    """All descendants with this tag name and attributes."""
    ...

def select(tag: Tag, selector: str) -> list[Tag]:
    """Descendants matching a CSS selector made of tag names, `.class`, `#id` and descendant combinators."""
    ...

def select_one(tag: Tag, selector: str) -> Tag | None: ...

def get(tag: Tag, key: str, default: str | None = None) -> str | None:
    """An attribute value."""
    ...

def get_text(tag: Tag, separator: str = '', strip: bool = False) -> str:
    """All text inside the tag."""
    ...

def record_model_info(model_information: dict[str, Any]) -> str:
    """Record one model. The dict needs `unique_id`, `name`, `input_mtok`, `output_mtok`
    (USD per million tokens, floats), optional `description` and `attributes`
    (a dict of extra facts such as `context_window`). Returns a success or validation message."""
    ...
'''

PROMPT = """
The variable `html` holds a provider's pricing page; it is far too large to print, so parse it with the
BeautifulSoup-like functions. Record every model listed in the pricing table with `record_model_info`, using the
row's `data-model` attribute as `unique_id`, the bold name, the description, the input and output prices in USD
per million tokens as floats, and `attributes={'context_window': <int tokens>}`. Skip deprecated models. The code
in `previous_code` extracted this page last time and may be reusable, but the page may have changed. Return the list
of unique ids you recorded.
"""

REFERENCE = """
page = beautiful_soup(html)
recorded = []
for row in select(page, 'table.pricing tbody tr'):
    cells = find_all(row, 'td')
    strong = find(cells[0], 'strong')
    desc = find(cells[0], 'span')
    if strong is None or desc is None:
        continue
    name = get_text(strong, strip=True)
    if 'deprecated' in name.lower():
        continue
    input_price = float(get_text(cells[1]).split('$')[1].split(' ')[0])
    output_price = float(get_text(cells[2]).split('$')[1].split(' ')[0])
    context = int(get_text(cells[3]).split(' ')[0].replace(',', ''))
    unique_id = get(row, 'data-model')
    record_model_info({
        'unique_id': unique_id,
        'name': name,
        'description': get_text(desc, strip=True),
        'input_mtok': input_price,
        'output_mtok': output_price,
        'attributes': {'context_window': context},
    })
    recorded.append(unique_id)
recorded
"""

TASK = Task(
    name='price_scrape',
    category='web',
    prompt=PROMPT.strip(),
    stubs=STUBS,
    tools={
        'beautiful_soup': beautiful_soup,
        'find': find,
        'find_all': find_all,
        'select': select,
        'select_one': select_one,
        'get': get,
        'get_text': get_text,
        'record_model_info': record_model_info,
    },
    inputs={'html': HTML, 'previous_code': PREVIOUS_CODE.strip()},
    expected=[uid for uid, name, *_ in MODELS if 'deprecated' not in name],
    evaluators=(Predicate('every current model recorded with the right prices', _recorded_matches),),
    reference_solution=REFERENCE,
    traps=('reusing previous_code unchanged', 'printing the html', 'str.format for prices'),
    max_result_bytes=200,
    setup=RECORDED.clear,
)
