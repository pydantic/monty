# `decimal` module

Monty's `decimal` is a port of CPython's pure-Python `_pydecimal` with the behaviour of the C `_decimal`
module CPython actually ships: arithmetic results, the flags each operation sets, which signal a trapped
operation raises and its `[<class 'decimal.DivisionByZero'>]` message all follow `_decimal`.
`Decimal`, `Context`, `localcontext()`, `getcontext()` / `setcontext()`, the three template contexts,
`IEEEContext()`, `DecimalTuple`, every signal class and every constant are available.

## Contexts

- There is one current context per session, not per thread or per `contextvars` context.
    `localcontext()` and `setcontext()` work as in CPython, and the current context persists across REPL
    feeds and snapshots like global variables do.
- `ctx.flags` and `ctx.traps` return a new mapping object on every access, so `ctx.flags is ctx.flags` is
    `False`; the mappings read and write through to the context, so everything else behaves the same.
    Their type prints as `<class 'SignalDict'>` (CPython: `<class 'abc.SignalDict'>`) and is not a
    `collections.abc.MutableMapping`.
    `keys()`, `values()` and `items()` return lists rather than views.
- `Context` objects cannot be pickled (`__reduce__` is absent); `copy.copy()` and `copy.deepcopy()` work.

## `Decimal`

- `Decimal` and `Context` cannot be subclassed (Monty has no class inheritance).
- A coefficient (or NaN payload) longer than 4300 digits is rejected when a `Decimal` is built from a
    `str`, `int` or tuple, with
    `ValueError: Exceeds the limit (4300 digits) for a Decimal coefficient: value has N digits`.
    The same applies to an `int` operand in arithmetic (`Decimal(1) + 10**5000`).
    CPython has no such limit.
    Results computed under a large `prec` are not capped, but materializing one past the memory limit
    raises `MemoryError`.
- `hash()` values differ from CPython's, as Monty's `int` and `float` hashes do; equal numbers still hash
    alike (`hash(Decimal('1.5')) == hash(1.5)`, `hash(Decimal(2)) == hash(2)`), except that an integral
    `Decimal` with more than 5000 digits does not hash like the equal `int`.
- A trapped signal's exception carries its message as a string: `exc.args` is
    `("[<class 'decimal.DivisionByZero'>]",)` where CPython's is a one-element tuple holding a list of
    classes. `str(exc)` matches.

## Formatting

- An f-string whose format spec is written literally, aligns, and starts its width with `0`, such as
    `f'{d:<020}'`, formats as if the spec were `0<20` instead of raising
    `ValueError: invalid format string`.
    The same spec passed at runtime (`format(d, '<020')`, `'{:<020}'.format(d)`) raises as in CPython.

## `json`

`json.loads(s, parse_float=Decimal)` works (see [json.md](json.md)); `json.dumps` rejects a `Decimal`
with `TypeError: Object of type Decimal is not JSON serializable`, as CPython does.
