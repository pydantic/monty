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
microsecond=0, tzinfo=None)`.
Attributes: `year`, `month`, `day`, `hour`, `minute`, `second`,
`microsecond`, `tzinfo`.
Methods: `isoformat`, `strftime`, `replace`, `weekday`, `isoweekday`,
`date`, `timestamp`.

Class methods supported: `now(tz=None)`, `strptime(date_string, format)`,
`fromisoformat(date_string)`.

- `now()` reaches the host for the current time (the only "live" datetime
  call); it yields an external call.
- `utcnow()` (the deprecated class method) and `today()` are not
  implemented.
- `combine()`, `fromtimestamp()`, `fromordinal()`, `utcfromtimestamp()`
  are not implemented.

Subclassing `datetime` is not possible (no `class` statement; see
[language.md](language.md)).

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
