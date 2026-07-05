# `decimal` module

Monty's `decimal.Decimal` is a Rust port of CPython's `_pydecimal` numeric
core, running under a **fixed context** — exactly CPython's default:
`prec=28`, `rounding=ROUND_HALF_EVEN`, `Emax=999999`, `Emin=-999999`,
`capitals=1`, `clamp=0`. Arithmetic results are digit-for-digit identical to
CPython's; the divergences are the missing context machinery, a reduced
method surface, and sandbox input caps.

## Module attributes

Implemented: `Decimal`, the full exception taxonomy (see *Exceptions*), and
the eight `ROUND_*` string constants.

Not implemented (`AttributeError`; `from decimal import …` raises
`ImportError`): `getcontext`, `setcontext`, `localcontext`, `Context`,
`DefaultContext`, `BasicContext`, `ExtendedContext`, `IEEEContext`,
`IEEE_CONTEXT_MAX_BITS`, `DecimalTuple`, `MAX_PREC`, `MAX_EMAX`, `MIN_EMIN`,
`MIN_ETINY`, `HAVE_THREADS`, `HAVE_CONTEXTVAR`.

## Fixed context

- `prec` cannot be changed — every operation rounds to 28 significant digits.
- The rounding mode is per-call, not global: `quantize` and
  `to_integral_value` accept CPython's `rounding=` argument (all eight
  modes); everything else — `__format__` included — always rounds
  `ROUND_HALF_EVEN`.
- No `context=` argument anywhere: methods that accept one in CPython and the
  `Decimal(value, context)` constructor raise `TypeError` — even for
  `Decimal('3', None)`, which CPython accepts — with generic arity/keyword
  wording rather than CPython's `optional argument must be a context`.
- No signal flags or traps. Trap behaviour is frozen at CPython's defaults:
  `InvalidOperation`, `DivisionByZero`, and `Overflow` always raise; the
  other signals never do (so float ↔ `Decimal` mixing in construction and
  comparison — CPython's armable `FloatOperation` — is always silent).

## Input size caps

CPython accepts unboundedly large constructor inputs; Monty caps *unrounded
constructor operands* (arithmetic results stay within `prec` and `Emax`
anyway):

- Coefficients and NaN payloads are capped at 4300 digits (the `int` ↔ `str`
  conversion limit) on every input path — string literals, `Decimal(int)`,
  the tuple form, host/snapshot input, and `int` operands promoted by
  arithmetic or methods — raising
  `ValueError: Decimal value exceeds the limit of 4300 digits`.
- `int(d)` / `hash(d)` of a huge-exponent integral value
  (`Decimal('1E+100000000')`) is charged to the resource tracker, so under a
  memory limit it raises `MemoryError` where CPython builds the integer.

Exponent literals are not additionally capped: the C module's own 64-bit
bounds apply, matching CPython.

## Construction

Accepts `Decimal(str | int | float | Decimal)` and
`Decimal((sign, digits, exponent))` (the `as_tuple` form), with `value`
usable as a keyword.

- `Decimal.from_float(x)` / `from_number(x)` are not implemented — call
  `Decimal(x)` directly for the identical exact value.
- ASCII digits only: non-ASCII decimal digits CPython accepts
  (`Decimal('٥')`) raise `InvalidOperation([ConversionSyntax])`.

## Methods

Implemented: `quantize`, `normalize`, `to_integral_value`, `sqrt`, `ln`,
`log10`, `exp`, `as_tuple`, `adjusted`, `copy_abs`, `copy_negate`,
`copy_sign`, the `is_*` predicates, plus the builtins `int()`, `float()`,
`round()`, `abs()`, `pow()` (2- and 3-argument), `divmod()`, `hash()`,
`__format__`, and the `math` module's `floor` / `ceil` / `trunc` /
float-consuming functions.

- `Decimal.__name__` (and `type(d).__name__`) is `'decimal.Decimal'`, not
  CPython's bare `'Decimal'` — consistent with Monty's qualified datetime
  type names.
- `as_tuple()` returns a named tuple with by-name and by-index access, but
  `DecimalTuple` itself is not importable (see *Module attributes*) and Monty
  has no per-class named-tuple identity (`type(x).__name__` is
  `'namedtuple'`); compare fields directly.
- Not implemented (raise `AttributeError`): `compare`, `compare_signal`,
  `compare_total`, `compare_total_mag`, `max`, `min`, `max_mag`, `min_mag`,
  `next_minus`, `next_plus`, `next_toward`, `number_class`, `radix`,
  `canonical`, `conjugate`, `copy`, `is_canonical`, `is_normal`,
  `is_subnormal`, `logb`, `logical_and`, `logical_or`, `logical_xor`,
  `logical_invert`, `rotate`, `scaleb`, `shift`, `fma`, `remainder_near`,
  `same_quantum`, `to_eng_string`, `to_integral_exact`, `from_float`,
  `from_number`, and the attributes `real` / `imag` / `as_integer_ratio`.

## Comparison and hashing

Comparisons against `int`, `bool`, `float`, and `Decimal` have CPython's
exact semantics (NaN and sNaN included), with these divergences:

- `Decimal(1) in range(3)` is `False` (CPython: `True`) — `range` membership
  has no `Decimal` fast path.
- Hashing uses Monty's runtime hash, not CPython's `_PyHASH_MODULUS`, so raw
  `hash(Decimal(...))` values differ from CPython but stay cross-type
  consistent in-sandbox: `hash(Decimal(5)) == hash(5)` and
  `hash(Decimal(f)) == hash(f)`, so equal numbers share a `dict`/`set` slot.

## Exceptions

The full CPython taxonomy is importable (`DecimalException`,
`InvalidOperation`, `ConversionSyntax`, `DivisionImpossible`,
`DivisionUndefined`, `InvalidContext`, `DivisionByZero`, `Overflow`,
`Inexact`, `Rounded`, `Subnormal`, `Clamped`, `Underflow`,
`FloatOperation`), with CPython's multi-parent relationships
(`FloatOperation` is a `TypeError`; `DivisionByZero` / `DivisionUndefined`
are `ZeroDivisionError`s; all are `ArithmeticError`s).

- Only `InvalidOperation`, `DivisionByZero`, and `Overflow` are ever raised
  (the default-trapped signals); the rest are catchable but, since traps
  cannot be armed, never raise in practice.
- `InvalidOperation` subtypes are never raised directly — like CPython, the
  condition appears in the *message* (`[<class 'decimal.ConversionSyntax'>]`),
  so `ConversionSyntax` etc. are caught only via `InvalidOperation`.
- `exc.args[0]` is that message *string*, not CPython's list of condition
  classes.
- `type(exc).__name__` is the qualified `'decimal.InvalidOperation'` etc.,
  not CPython's bare `'InvalidOperation'` — the same pattern as
  `Decimal.__name__` above.

## Formatting

`str()`, `repr()`, and `__format__` follow CPython's spec mini-language for
`Decimal` (including `n`, which behaves as `g` — Monty has no locale), with
one divergence: the `e`/`E`/`f`/`%` presentations charge fraction-padding to
the resource tracker, so an absurd precision (`f'{Decimal("1"):.{10**9}e}'`)
raises `MemoryError` under a memory limit where CPython builds the giant
string.

## Host boundary

A `Decimal` crosses the Monty ↔ host boundary losslessly as its canonical
string (exposed by `pydantic_monty` as `decimal.Decimal`, by
`@pydantic/monty` as a tagged string).

- Boundary equality/hashing is string-based: `Decimal('1.2')` and
  `Decimal('1.20')` are *distinct* at the boundary though equal in-sandbox.
- A host `Decimal` over the 4300-digit cap (see *Input size caps*) is
  rejected crossing into the sandbox, not truncated.
- A host-raised `decimal.*` exception re-enters the sandbox as its nearest
  builtin ancestor (`ArithmeticError`, or `TypeError` for `FloatOperation`),
  so `except decimal.Underflow:` cannot catch it; sandbox-raised `decimal`
  exceptions surfacing to the host keep their exact class.
