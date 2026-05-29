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

Constructor: `timedelta(days=0, seconds=0, microseconds=0, minutes=0,
hours=0)`. The CPython `milliseconds` and `weeks` parameters are not
supported.
Attributes: `days`, `seconds`, `microseconds`.
No methods (`.total_seconds()` is **not** implemented).

Arithmetic (`+`, `-`, `*`, comparisons) works between `timedelta`s and
between `datetime`/`date` and `timedelta`. Division and floor-division of
two `timedelta`s is not implemented.

## `timezone`

Constructor: `timezone(offset, name=None)` where `offset` is a
`timedelta`.
Attributes: `offset`, `name`.

`timezone.utc` and `timezone.min` / `timezone.max` class constants are not
defined. The abstract `tzinfo` base class is not exposed.

## Formatting

`strftime` supports the directives that map onto Rust's `chrono`
formatting; locale-specific directives (`%c`, `%x`, `%X`, `%p`) follow
Rust's defaults rather than the C locale and may differ from CPython.
`%Z` always emits an empty string for naive datetimes.
