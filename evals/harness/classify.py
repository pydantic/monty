"""Turns a Monty failure into a feature-gap record, or decides it is not one.

This is the half of the suite that answers "what should we build next". The
discrimination that matters is between a failure caused by Monty missing something
(`NameError: name 'functools' is not defined`) and a failure caused by the model
writing bad code (`NameError: name 'totl' is not defined`). Only the first is a gap,
and only the tables in `gaps.py` can tell them apart.

Every rule here was checked against the built worker rather than inferred from the
docs — Monty's error shapes are not all what `limitations/` implies. Two that matter:
parse-time rejections surface as `MontyRuntimeError` wrapping `NotImplementedError`,
not as `MontySyntaxError`; and a missing module attribute is sometimes reported with
the module named (`module 'itertools' has no attribute 'groupby'`) and sometimes not
(`'module' object has no attribute 'sleep'`).

Anything that cannot be placed lands in `unclassified` with its message intact. That
bucket is meant to stay small; when it grows, the classifier needs a new rule, not a
wider net.
"""

from __future__ import annotations

import re
from dataclasses import dataclass

from pydantic_monty import MontyError, MontyRuntimeError, MontySyntaxError, MontyTypingError

from .gaps import (
    CONSTRUCT_PREFIX,
    MISSING_BUILTINS,
    MISSING_METHODS,
    MISSING_MODULE_MEMBERS,
    MISSING_MODULES,
    NOT_YET_SUPPORTED,
    limitation_doc,
)

__all__ = ('FeatureGap', 'classify')

_NAME_ERROR = re.compile(r"name '([^']+)' is not defined")
_NO_MODULE = re.compile(r"No module named '([^']+)'")
_CANNOT_IMPORT = re.compile(r"cannot import name '([^']+)' from '([^']+)'")
_NAMED_MODULE_ATTR = re.compile(r"module '([^']+)' has no attribute '([^']+)'")
_TYPE_ATTR = re.compile(r"'([^']+)' object has no attribute '([^']+)'")
_UNEXPECTED_KWARG = re.compile(r"unexpected keyword argument '([^']+)'")
_ARITY = re.compile(r'(\w+)\(?\)? ?expected \d+ arguments?, got \d+')
_UNSUPPORTED_OPERAND = re.compile(r"unsupported operand type\(s\) for ([^:]+): '([^']+)' and '([^']+)'")
_DOTTED_ACCESS = re.compile(r'\b([a-z_][a-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)')


@dataclass(frozen=True)
class FeatureGap:
    """One thing Monty could not do, attributed to a symbol and a source line.

    `kind` drives how the ledger groups rows; `symbol` is what gets ranked. `certain`
    marks gaps confirmed against the `gaps.py` tables — an uncertain gap is a guess
    from the error text alone and should be triaged by hand before it drives roadmap
    decisions.
    """

    kind: str
    symbol: str
    message: str
    source_line: str | None = None
    certain: bool = True

    @property
    def doc(self) -> str | None:
        """The `limitations/` file this gap should be documented in, if any."""
        return limitation_doc(self.kind, self.symbol)


def classify(error: MontyError) -> FeatureGap | None:
    """Map a Monty failure to a feature gap, or `None` if the model is simply wrong.

    Returning `None` is a real answer, not a fallback: most failures in a good run are
    ordinary bugs in generated code, and counting those as missing features would make
    the ledger useless.
    """
    message = _message(error)
    source_line = _source_line(error)

    if isinstance(error, MontyTypingError):
        return FeatureGap('type_check', _type_check_symbol(message), message, source_line)

    if isinstance(error, MontySyntaxError):
        # Monty reports unsupported *constructs* as NotImplementedError at runtime;
        # a genuine MontySyntaxError means the model emitted invalid Python.
        return None

    if isinstance(error, MontyRuntimeError):
        return _classify_runtime(error, message, source_line)

    return FeatureGap('unclassified', type(error).__name__, message, source_line, certain=False)


def _classify_runtime(error: MontyRuntimeError, message: str, source_line: str | None) -> FeatureGap | None:
    """Classify a runtime failure by its inner exception type and message shape."""
    exc_name = _inner_exception_name(error)

    if exc_name in {'MemoryError', 'TimeoutError'}:
        return FeatureGap('resource', exc_name, message, source_line)

    if exc_name == 'NotImplementedError':
        return FeatureGap('construct', _construct(message), message, source_line)

    if exc_name in {'ImportError', 'ModuleNotFoundError'}:
        return _classify_import(message, source_line)

    if exc_name == 'NameError':
        return _classify_name(message, source_line)

    if exc_name == 'AttributeError':
        return _classify_attribute(message, source_line)

    if exc_name == 'TypeError':
        return _classify_type_error(message, source_line)

    return None


def _classify_name(message: str, source_line: str | None) -> FeatureGap | None:
    """A `NameError` is a gap only when the name is one Monty is known to lack."""
    match = _NAME_ERROR.search(message)
    if match is None:
        return None
    name = match.group(1)
    if name in MISSING_BUILTINS:
        return FeatureGap('builtin', name, message, source_line)
    if name in MISSING_MODULES:
        return FeatureGap('module', name, message, source_line)
    # An undefined name of the model's own invention — its bug, not ours.
    return None


def _classify_import(message: str, source_line: str | None) -> FeatureGap | None:
    """Attribute an import failure to a whole missing module or one missing member."""
    match = _CANNOT_IMPORT.search(message)
    if match is not None:
        name, module = match.group(1), match.group(2)
        certain = name in MISSING_MODULE_MEMBERS.get(module, frozenset())
        return FeatureGap('module_member', f'{module}.{name}', message, source_line, certain=certain)

    match = _NO_MODULE.search(message)
    if match is not None:
        module = match.group(1)
        return FeatureGap('module', module, message, source_line, certain=module in MISSING_MODULES)

    return FeatureGap('module', 'unknown', message, source_line, certain=False)


def _classify_attribute(message: str, source_line: str | None) -> FeatureGap | None:
    """Split `AttributeError` into a missing module member and a missing type method.

    Both are real gaps, but they rank differently: a missing module member is a hole
    in a module we ship, a missing method is a hole in a type we ship.
    """
    match = _NAMED_MODULE_ATTR.search(message)
    if match is not None:
        module, name = match.group(1), match.group(2)
        certain = name in MISSING_MODULE_MEMBERS.get(module, frozenset())
        return FeatureGap('module_member', f'{module}.{name}', message, source_line, certain=certain)

    match = _TYPE_ATTR.search(message)
    if match is not None:
        type_name, attr = match.group(1), match.group(2)
        if type_name == 'module':
            # Monty omits the module name in this form; recover it from the source
            # line so the ledger can still rank `asyncio.sleep` rather than `?.sleep`.
            module = _module_from_source(source_line, attr) or '?'
            certain = attr in MISSING_MODULE_MEMBERS.get(module, frozenset())
            return FeatureGap('module_member', f'{module}.{attr}', message, source_line, certain=certain)
        symbol = f'{type_name}.{attr}'
        return FeatureGap('method', symbol, message, source_line, certain=symbol in MISSING_METHODS)

    return FeatureGap('unclassified', 'AttributeError', message, source_line, certain=False)


def _classify_type_error(message: str, source_line: str | None) -> FeatureGap | None:
    """Pick the deliberate limitations out of `TypeError`, which is mostly model error.

    Four shapes are gaps: an explicit "not yet supported", an unsupported keyword
    argument, `str % ...` (Monty implements no percent formatting), and an arity
    complaint from a function whose extra parameters are keyword-only in Monty but
    positional in CPython (`sorted`).
    """
    if NOT_YET_SUPPORTED in message:
        return FeatureGap('argument', _trim(message), message, source_line)

    match = _UNEXPECTED_KWARG.search(message)
    if match is not None:
        return FeatureGap('argument', match.group(1), message, source_line, certain=False)

    match = _UNSUPPORTED_OPERAND.search(message)
    if match is not None and match.group(1).strip() == '%' and match.group(2) == 'str':
        return FeatureGap('operator', 'str %', message, source_line)

    match = _ARITY.search(message)
    if match is not None:
        return FeatureGap('argument', f'{match.group(1)}() arity', message, source_line, certain=False)

    return None


def _construct(message: str) -> str:
    """Name the construct Monty refused, from its stable rejection prefix.

    Falls back to the trimmed message when the prefix is absent, which happens for
    `NotImplementedError`s raised outside the parser (unsupported `@dataclass`
    options, `os` calls from a native callback).
    """
    lowered = message.lower()
    index = lowered.find(CONSTRUCT_PREFIX)
    if index == -1:
        return _trim(message)
    tail = message[index + len(CONSTRUCT_PREFIX) :].strip()
    # Messages often qualify the construct in parentheses; the head is the symbol.
    return _trim(tail.split('(')[0].strip().rstrip('.'))


def _module_from_source(source_line: str | None, attr: str) -> str | None:
    """Recover `x` from a `x.attr` access on the failing source line."""
    if not source_line:
        return None
    for module, name in _DOTTED_ACCESS.findall(source_line):
        if name == attr:
            return module
    return None


def _type_check_symbol(message: str) -> str:
    """Reduce a `ty` diagnostic to its rule name, which is what the ledger groups by."""
    rule = re.search(r'\[([a-z-]+)\]', message)
    return rule.group(1) if rule else 'type-error'


def _trim(message: str, limit: int = 60) -> str:
    """Shorten a message to a stable grouping key."""
    cleaned = message.split(':', 1)[-1].strip() if ':' in message else message.strip()
    return cleaned if len(cleaned) <= limit else f'{cleaned[:limit].rstrip()}…'


def _inner_exception_name(error: MontyError) -> str:
    """Name of the Python exception the sandbox actually raised.

    `MontyError.exception()` hands back a real exception object, which is far more
    reliable to branch on than parsing the rendered message.
    """
    try:
        return type(error.exception()).__name__
    except Exception:  # noqa: BLE001 - fall back to text when the inner value cannot be built
        return ''


def _message(error: MontyError) -> str:
    """The `ExceptionType: message` line, falling back to `str()`.

    `MontyTypingError.display()` takes no format argument, unlike the other two, so
    it is handled by type rather than by trying and catching.
    """
    if isinstance(error, MontyTypingError):
        return error.display()
    if isinstance(error, (MontyRuntimeError, MontySyntaxError)):
        return error.display('type-msg')
    return str(error)


def _source_line(error: MontyError) -> str | None:
    """The deepest traceback frame's source line — where the gap was actually hit."""
    if not isinstance(error, (MontyRuntimeError, MontySyntaxError)):
        return None
    for frame in reversed(error.traceback()):
        line = frame.source_line
        if line:
            return line.strip()
    return None
