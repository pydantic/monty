"""Tables of what Monty does not implement, used to classify failures.

These exist to answer one question the raw error text cannot: is a `NameError` a
Monty gap, or did the model just reference a variable it never defined? Only names
in these tables count as gaps; everything else is scored as the model's own bug.

Sourced from `limitations/` — `builtins.md`, `modules.md`, `itertools.md`,
`collections.md`, `math.md`, `asyncio.md`, `json.md`, `re.md`, `sys.md`,
`datetime.md`, `typing.md`. When a limitation is lifted, delete the entry here in the
same change, or the ledger will keep reporting a gap that no longer exists.
"""

from __future__ import annotations

__all__ = (
    'CONSTRUCT_PREFIX',
    'MISSING_BUILTINS',
    'MISSING_METHODS',
    'MISSING_MODULES',
    'MISSING_MODULE_MEMBERS',
    'NOT_YET_SUPPORTED',
    'limitation_doc',
)

MISSING_BUILTINS = frozenset(
    {
        'aiter',
        'anext',
        'ascii',
        'bytearray',
        'callable',
        'classmethod',
        'compile',
        'complex',
        'delattr',
        'dir',
        'eval',
        'exec',
        'format',
        'globals',
        'help',
        'input',
        'issubclass',
        'locals',
        'memoryview',
        'object',
        'property',
        'staticmethod',
        'super',
        'vars',
        '__import__',
    }
)
"""Builtins CPython has and Monty does not. See `limitations/builtins.md`."""

MISSING_MODULES = frozenset(
    {
        'abc',
        'argparse',
        'array',
        'base64',
        'bisect',
        'calendar',
        'contextlib',
        'copy',
        'csv',
        'ctypes',
        'decimal',
        'difflib',
        'enum',
        'fractions',
        'functools',
        'glob',
        'hashlib',
        'heapq',
        'io',
        'inspect',
        'logging',
        'multiprocessing',
        'numbers',
        'operator',
        'pickle',
        'pprint',
        'queue',
        'random',
        'secrets',
        'shutil',
        'socket',
        'sqlite3',
        'statistics',
        'string',
        'struct',
        'subprocess',
        'tempfile',
        'textwrap',
        'threading',
        'time',
        'traceback',
        'types',
        'urllib',
        'uuid',
        'warnings',
        'weakref',
        'zoneinfo',
    }
)
"""Stdlib modules Monty does not bundle. See `limitations/modules.md`."""

MISSING_MODULE_MEMBERS: dict[str, frozenset[str]] = {
    'asyncio': frozenset(
        {'Event', 'Future', 'Lock', 'Queue', 'TaskGroup', 'create_task', 'sleep', 'timeout', 'to_thread', 'wait_for'}
    ),
    'collections': frozenset({'ChainMap', 'OrderedDict', 'UserDict', 'UserList', 'UserString', 'abc'}),
    'itertools': frozenset(
        {'accumulate', 'batched', 'combinations', 'groupby', 'permutations', 'product', 'tee', 'zip_longest'}
    ),
    'json': frozenset({'dump', 'load'}),
    'math': frozenset({'dist', 'fsum', 'hypot', 'prod', 'sumprod'}),
    're': frozenset({'subn'}),
    'sys': frozenset({'argv', 'exc_info', 'exit', 'getrecursionlimit', 'maxsize', 'modules', 'path', 'stdin'}),
    # `timezone.utc` and `datetime.strptime` are present despite what a reading of
    # `limitations/datetime.md` suggests — both verified against the built worker.
    'datetime': frozenset({'time', 'tzinfo'}),
    'dataclasses': frozenset({'InitVar', 'MISSING', 'asdict', 'astuple', 'field', 'fields', 'replace'}),
    'typing': frozenset(
        {
            'NamedTuple',
            'NewType',
            'ParamSpec',
            'TypeAlias',
            'TypedDict',
            'cast',
            'final',
            'get_args',
            'get_origin',
            'get_type_hints',
            'overload',
            'runtime_checkable',
        }
    ),
}
"""Names absent from modules Monty *does* bundle — the sharpest class of gap.

A model has no way to guess these: the module imports fine, so the failure only
appears at the point of use.
"""

MISSING_METHODS = frozenset(
    {
        'str.format',
        'str.format_map',
        'str.translate',
        'str.maketrans',
    }
)
"""Methods on types Monty ships that CPython has and Monty does not.

Kept short deliberately: an `AttributeError` on a method not listed here is far more
likely to be the model inventing a method than a gap worth tracking.
"""

CONSTRUCT_PREFIX = 'the monty syntax parser does not yet support '
"""Stable prefix on Monty's parse-time rejections; the tail names the construct.

Parsing the tail beats substring-matching a keyword list — `'del'` alone would also
fire on `'model'`, and the phrasing of these messages is not an API we control.
"""

NOT_YET_SUPPORTED = 'not yet supported'
"""Marks a deliberate Monty limitation reported through an ordinary exception type.

`re.sub` with a callable replacement raises `TypeError` carrying this phrase; without
it the failure would be indistinguishable from the model passing a bad argument.
"""

_DOC_BY_KIND = {
    'construct': 'limitations/language.md',
    'builtin': 'limitations/builtins.md',
    'module': 'limitations/modules.md',
    'type_check': 'limitations/typing.md',
    'resource': 'limitations/resource_limits.md',
}

_DOC_BY_SYMBOL = {
    'str %': 'limitations/format.md',
    'str.format': 'limitations/format.md',
    'str.format_map': 'limitations/format.md',
    'str.translate': 'limitations/format.md',
    'str.maketrans': 'limitations/format.md',
}
"""Gaps whose documentation lives somewhere the kind alone would not predict."""


def limitation_doc(kind: str, symbol: str) -> str | None:
    """Best-guess `limitations/` file documenting a gap, for citation in the ledger.

    Module-member gaps map to the module's own doc; a handful of symbols are mapped
    explicitly; everything else falls back to a per-kind default. `None` means no good
    guess — a hint that the divergence may not be documented at all, which is itself
    worth knowing when triaging the ledger.
    """
    if symbol in _DOC_BY_SYMBOL:
        return _DOC_BY_SYMBOL[symbol]
    if kind == 'module_member':
        module = symbol.split('.', 1)[0]
        return f'limitations/{module}.md'
    if kind == 'argument' and 're.sub' in symbol:
        return 'limitations/re.md'
    return _DOC_BY_KIND.get(kind)
