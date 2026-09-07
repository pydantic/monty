# `datetime` module

Provides five classes: `date`, `datetime`, `time`, `timedelta`,
`timezone`. The module-level `tzinfo`, `MINYEAR` / `MAXYEAR` symbols are
not exposed.

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
while 64-bit-`long` platforms raise the sign-aware `signed integer is greater than maximum` / `less than minimum`.
Monty's ints are i64 on
every host, so it always uses the 64-bit wording, matching CPython on
Linux/macOS but not on Windows. Same for `datetime`; values wider than
i64 raise the `C long` message on all platforms, matching CPython.

## `datetime`

Constructor: `datetime(year, month, day, hour=0, minute=0, second=0, microsecond=0, tzinfo=None, *, fold=0)`. `fold` is
accepted and validated
(must be 0 or 1) for CPython argument-parsing parity but does not affect
the stored value: Monty does not track DST-fold disambiguation.
Attributes: `year`, `month`, `day`, `hour`, `minute`, `second`,
`microsecond`, `tzinfo`.
Methods: `isoformat`, `strftime`, `replace`, `weekday`, `isoweekday`,
`date`, `time`, `timetz`, `timestamp`.

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
- `time()` and `timetz()` return a `time` whose `fold` is always 0, since
    `datetime` does not store the flag (above).

Subclassing `datetime` is not possible, since there is no class inheritance
(see [classes.md](classes.md)).

`datetime.replace()`, `date.replace()` and `time.replace()` accept **only
keyword arguments** in Monty. CPython accepts positional args too
(`d.replace(2025)` is valid in CPython 3.14). Calling with positionals
in Monty raises `TypeError: replace expected at most 0 arguments, got N`.

## `time`

Constructor: `time(hour=0, minute=0, second=0, microsecond=0, tzinfo=None, *, fold=0)`.
Attributes: `hour`, `minute`, `second`, `microsecond`, `tzinfo`, `fold`.
Methods: `isoformat(timespec='auto')`, `strftime`, `replace`,
`utcoffset`, `tzname`, `dst`.
Class methods: `fromisoformat(time_string)`. `strptime(string, format)`
— which CPython added in 3.14 — is not implemented and raises
`AttributeError`.

The `min`, `max` and `resolution` class constants are not defined, and
raise `AttributeError`. Type checking will not warn you: it resolves
`time` against typeshed, which declares them, so `monty -t` passes on
`time.min` and the lookup fails at runtime.

`fromisoformat()` parses with [speedate](https://docs.rs/speedate), the
same parser `date` and `datetime` use, so it accepts a narrower grammar
than CPython 3.11+. The compact form (`'123005'`), a leading `T`
(`'T12:30'`), a sub-minute UTC offset (`'12:30:05+01:00:30'`) and more
than 6 fractional-second digits (`'12:30:05.1234567'`) are all rejected
with `ValueError: Invalid isoformat string: '...'`. CPython instead names
the offending component of a syntactically valid but out-of-range string:
`time.fromisoformat('25:00')` raises `hour must be in 0..23, not 25`
there.

`tzinfo` accepts only `None` or a built-in `timezone` instance. The
`tzinfo` ABC is not implemented, so custom subclasses are rejected, the
same restriction as `datetime`.

`fold` is stored and reported by `.fold` and `repr()`, and survives the host
boundary, but is never read: Monty has no DST model, so it cannot use the flag
to pick between the two readings of a repeated wall clock. As in CPython, `fold`
is excluded from `==` and `hash()`, and omitted by `isoformat()`.

Ordering an aware `time` against a naive one raises
`TypeError: '<' not supported between instances of 'datetime.time' and 'datetime.time'`, where CPython raises
`TypeError: can't compare offset-naive and offset-aware times`. `==` returns `False` without
raising, matching CPython. The same wording divergence applies to
`datetime`.

A host `datetime.time` carrying a `tzinfo` that is not a `datetime.timezone`
(a `ZoneInfo`, say) is rejected with `cannot convert datetime.time with tzinfo of type '...' to a Monty value`. A bare
time has no instant to resolve
a named zone against — CPython's own `t.utcoffset()` returns `None` there —
so there is no offset to carry. An aware `datetime` is not affected: it has a
date, and its zone resolves through `utcoffset(dt)`.

## `timedelta`

Constructor: `timedelta(days=0, seconds=0, microseconds=0, *, milliseconds=0, minutes=0, hours=0, weeks=0)`.
`milliseconds`,
`minutes`, `hours`, and `weeks` are keyword-only in Monty; CPython accepts
all seven positionally.
Attributes: `days`, `seconds`, `microseconds`.
Methods: `total_seconds`.

A non-int component raises `TypeError: '{type}' object cannot be interpreted as an integer`; CPython names the offending
component instead
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
Monty, but the type error in CPython (`timezone() argument 1 must be datetime.timedelta, not str`). CPython's parser
type-checks `offset` while
binding, whereas Monty validates the `timedelta` in the constructor body
after binding completes.

## Formatting

`strftime` supports the directives that map onto Rust's `chrono`
formatting; locale-specific directives (`%c`, `%x`, `%X`, `%p`) follow
Rust's defaults rather than the C locale and may differ from CPython.

### Unrecognised directives

An **unrecognised directive is passed through verbatim**, matching glibc/Linux
CPython (`strftime('%Q') == '%Q'`, `strftime('%') == '%'`). This is a choice
of *one* CPython, not all of them: macOS CPython instead drops the `%`
(`strftime('%Q') == 'Q'`), because unknown-directive handling is delegated to
the platform C library and is genuinely platform-dependent. The same
pass-through applies to f-string and `str.format()` formatting (below).

### Directives that need data the value lacks

A directive that is *recognised* but can't be rendered for the given value
raises `ValueError: Invalid format string` rather than substituting a default
the way CPython does. The known cases:

- Time directives (`%H`, `%M`, `%S`, `%p`, …) on a bare `date`: Monty stores a
    `date` with no time component, so these raise; CPython fills zeros (`'00'`,
    `'AM'`).
- `%z` / `%Z` on a naive `date`, `datetime` or `time`: Monty raises; CPython
    yields `''`.
- `%z` / `%Z` on an **aware** `datetime` or `time`: Monty formats the wall-clock
    (naive) components and so raises rather than emitting the offset/name; CPython
    yields `'+0200'` / `'CEST'`. Threading the timezone through formatting is not
    yet implemented.

f-strings and `str.format()` format `date`, `datetime` and `time` values through
`strftime`, matching CPython's `__format__`: `f'{dt:%Y-%m-%d}'` and
`'{:%Y-%m-%d}'.format(dt)` are equivalent to `dt.strftime('%Y-%m-%d')`, and
an empty spec uses `str(dt)`. One edge-case divergence remains for a literal
f-string spec that is also a valid format mini-language spec (e.g.
`f'{dt:>10}'` or a lone `f'{dt:%}'`): Monty applies generic string formatting,
where CPython treats the entire spec as a `strftime` string. Dynamically built
f-string specs and `str.format()` specs are handed to `strftime`.
