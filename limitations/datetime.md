# `datetime` module

Provides four classes: `date`, `datetime`, `timedelta`, `timezone`. The
module-level `time`, `tzinfo`, and `MINYEAR` / `MAXYEAR` symbols are not
exposed.

## `date`

Constructor: `date(year, month, day)`.
Attributes: `year`, `month`, `day`.
Methods: `isoformat`, `strftime`, `replace`, `weekday`, `isoweekday`.

Class methods `today()`, `fromisoformat()`, `fromisocalendar()`,
`fromtimestamp()`, `fromordinal()` are not implemented. `today()` is
missing because the sandbox has no access to the host clock.

Constructor overflow wording on Windows: CPython's `i` converter goes
through C `long`, which is 32 bits on Windows, so `date(2**40, 1, 1)`
raises `OverflowError: Python int too large to convert to C long` there,
while 64-bit-`long` platforms raise the sign-aware `signed integer is
greater than maximum` / `less than minimum`. Monty's ints are i64 on
every host, so it always uses the 64-bit wording — matching CPython on
Linux/macOS but not on Windows. (Same for `datetime`; values wider than
i64 raise the `C long` message on all platforms, matching CPython.)

## `datetime`

Constructor: `datetime(year, month, day, hour=0, minute=0, second=0,
microsecond=0, tzinfo=None, *, fold=0)`. `fold` is accepted and validated
(must be 0 or 1) for CPython argument-parsing parity but does not affect
the stored value — Monty does not track DST-fold disambiguation.
Attributes: `year`, `month`, `day`, `hour`, `minute`, `second`,
`microsecond`, `tzinfo`.
Methods: `isoformat`, `strftime`, `replace`, `weekday`, `isoweekday`,
`date`, `timestamp`.

Class methods supported: `now(tz=None)`, `strptime(date_string, format)`,
`fromisoformat(date_string)`.

- `now()` reaches the host for the current time (the only "live" datetime
  call); it yields an external call.
- `now(tz)` returns a `datetime` whose `tzinfo` is `==` the input timezone
  but not `is` it: the original `tzinfo` object isn't threaded through the
  OS-call resume, so a fresh `timezone` is reconstructed from the
  offset/name on the return path.
- `utcnow()` (the deprecated class method) and `today()` are not
  implemented.
- `combine()`, `fromtimestamp()`, `fromordinal()`, `utcfromtimestamp()`
  are not implemented.

Subclassing `datetime` is not possible (no `class` statement; see
[language.md](language.md)).

`datetime.replace()` and `date.replace()` accept **only keyword
arguments** in Monty. CPython accepts positional args too
(`d.replace(2025)` is valid in CPython 3.14). Calling with positionals
in Monty raises `TypeError: replace expected at most 0 arguments,
got N`.

## `timedelta`

Constructor: `timedelta(days=0, seconds=0, microseconds=0, *,
milliseconds=0, minutes=0, hours=0, weeks=0)` — note `milliseconds`,
`minutes`, `hours`, and `weeks` are keyword-only in Monty; CPython accepts
all seven positionally.
Attributes: `days`, `seconds`, `microseconds`.
Methods: `total_seconds`.

A non-int component raises `TypeError: '{type}' object cannot be
interpreted as an integer`; CPython names the offending component instead
(`unsupported type for timedelta days component: str`).

Arithmetic (`+`, `-`, `*`, comparisons) works between `timedelta`s and
between `datetime`/`date` and `timedelta`. Division and floor-division of
two `timedelta`s is not implemented.

## `timezone`

Constructor: `timezone(offset, name=None)` where `offset` is a
`timedelta`.
Attributes: `offset`, `name`.

`timezone.utc` and `timezone.min` / `timezone.max` class constants are not
defined. The abstract `tzinfo` base class is not exposed.

One error-ordering corner: `timezone('x', offset=td)` (a non-`timedelta`
positional *and* an `offset` kwarg) raises the name-and-position conflict in
Monty, but the type error in CPython (`timezone() argument 1 must be
datetime.timedelta, not str`) — CPython's parser type-checks `offset` while
binding, whereas Monty validates the `timedelta` in the constructor body
after binding completes.

## Formatting

`strftime` supports the directives that map onto Rust's `chrono`
formatting; locale-specific directives (`%c`, `%x`, `%X`, `%p`) follow
Rust's defaults rather than the C locale and may differ from CPython.

### Unrecognised directives

An **unrecognised directive is passed through verbatim**, matching glibc/Linux
CPython (`strftime('%Q') == '%Q'`, `strftime('%') == '%'`). Note this is a
deliberate choice of *one* CPython, not all of them: macOS CPython instead
drops the `%` (`strftime('%Q') == 'Q'`), because unknown-directive handling is
delegated to the platform C library and is genuinely platform-dependent. The
same pass-through applies to f-string formatting (below).

### Directives that need data the value lacks

A directive that is *recognised* but can't be rendered for the given value
raises `ValueError: Invalid format string` rather than substituting a default
the way CPython does. The known cases:

- Time directives (`%H`, `%M`, `%S`, `%p`, …) on a bare `date`: Monty stores a
  `date` with no time component, so these raise; CPython fills zeros (`'00'`,
  `'AM'`).
- `%z` / `%Z` on a naive `date`/`datetime`: Monty raises; CPython yields `''`.
- `%z` / `%Z` on an **aware** `datetime`: Monty formats the wall-clock (naive)
  components and so raises rather than emitting the offset/name; CPython yields
  `'+0200'` / `'CEST'`. (Threading the timezone through formatting is not yet
  implemented.)

f-strings format `date`/`datetime` values through `strftime`, matching
CPython's `__format__`: `f'{dt:%Y-%m-%d}'` is equivalent to
`dt.strftime('%Y-%m-%d')`, and an empty spec (`f'{dt}'` or `f'{dt:}'`) uses
`str(dt)`. One edge-case divergence: a spec that also happens to be a valid
format mini-language spec (e.g. `f'{dt:>10}'` or a lone `f'{dt:%}'`) is
applied as generic string formatting rather than handed to `strftime` —
CPython treats the *entire* spec as a `strftime` string. Real strftime specs
(those containing `%` directives like `%Y`) are unaffected.
