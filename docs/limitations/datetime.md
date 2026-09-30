# `datetime` module

Provides five classes: `date`, `datetime`, `time`, `timedelta`,
`timezone`. The module-level `tzinfo`, `MINYEAR` / `MAXYEAR` symbols are
not exposed.

Error messages name `date`, `timedelta` and `timezone` without their
module, where CPython qualifies them.
`time(0, tzinfo=timedelta(hours=1))` reports `not type 'timedelta'`,
`time(1) < date(2020, 1, 1)` ends `and 'date'`, and
`datetime.combine(date(2020, 1, 1), date(2020, 1, 1))` reports
`not date` — CPython prefixes `datetime.` to each.
`datetime` and `time` are qualified.
Passing `None` where an integer component is expected reports `'None'`
rather than CPython's `'NoneType'` (`time(None)`, `datetime(None, 1, 1)`).

## Class constants

`min`, `max` and `resolution` are defined on `date`, `datetime`, `time`
and `timedelta`, and `min` / `max` / `utc` on `timezone`. CPython caches
each constant, so `date.min is date.min` is `True` there and `False`
here: every access allocates a new object. Equality and ordering are
unaffected. `timezone.utc` is the exception — it is a singleton on both,
so `timezone.utc is timezone.utc` holds.

## `fromisoformat`

`date.fromisoformat`, `datetime.fromisoformat` and `time.fromisoformat`
all parse with [speedate](https://docs.rs/speedate), which accepts a
narrower grammar than CPython 3.11+:

- The compact forms are rejected (`time.fromisoformat('123005')`,
    `datetime.fromisoformat('20200101T123005')`).
- A leading `T` is rejected (`time.fromisoformat('T12:30')`).
- Sub-minute UTC offsets are rejected
    (`'12:30:05+01:00:30'`), even though the same offset is accepted from
    the `timezone` constructor.
- More than 6 fractional-second digits are rejected rather than
    truncated (`'12:30:05.1234567'`).

Every rejection raises `ValueError: Invalid isoformat string: '...'`.
CPython instead reports the offending component for a syntactically
valid string with an out-of-range value: `time.fromisoformat('25:00')`
raises `hour must be in 0..23, not 25` there, and
`'12:30:05+99:00'` raises the `offset must be a timedelta strictly between ...` message.

## `date`

Constructor: `date(year, month, day)`.
Attributes: `year`, `month`, `day`.
Methods: `isoformat`, `strftime`, `replace`, `weekday`, `isoweekday`.

Class methods supported: `today()`, `fromisoformat()`.
`fromisocalendar()`, `fromtimestamp()`, `fromordinal()` are not
implemented. `strptime()`, added in CPython 3.14, is not implemented
either.

`today()` reads the clock, so it depends on the embedder granting one —
see "Reading the clock" below.

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
Methods: `isoformat(sep='T', timespec='auto')`, `strftime`, `replace`,
`weekday`, `isoweekday`, `date`, `time`, `timetz`, `timestamp`, `astimezone(tz=None)`,
`utcoffset`, `tzname`, `dst`.

`astimezone()` and a naive `timestamp()` read the [session zone](#reading-the-clock) where CPython uses the host's;
as in CPython the `tzinfo` `astimezone()` attaches is a `timezone` carrying the offset and abbreviation at that
instant (`BST`).
The default zone is `timezone(timedelta(0), 'UTC')`, what CPython reports under `TZ=UTC`, so
`datetime.now().astimezone().tzinfo == timezone.utc` holds but it is not the `timezone.utc` singleton.
A naive value is read in the session zone, as CPython reads it in the host's, at its first occurrence in a DST fold
and with the offset from before a DST gap: CPython's `fold=0` reading, since `fold` is not stored (below).
`dt.astimezone(dt.tzinfo)` returns an equal copy, not `dt` itself.

`fold` is not readable: `datetime(2020, 1, 1, fold=1).fold` raises
`AttributeError`, where CPython returns `1`.

Class methods supported: `now(tz=None)`, `strptime(date_string, format)`,
`fromisoformat(date_string)`, `combine(date, time, tzinfo=self.tzinfo)`.

- `now()` reads the clock — see "Reading the clock" below.
- Host-answered `now(tz)` reconstructs `tzinfo` from its offset and name, so it equals `tz` but is a different object.
    The default sandbox answer preserves identity, as CPython does.
- `strptime()` requires the string to carry a date: a time-only format
    (`strptime('12:30', '%H:%M')`) raises
    `ValueError: time data '12:30' does not match format '%H:%M'`, where
    CPython defaults the missing date to 1900-01-01.
- `strptime()`'s `%z` takes a sign, hours and minutes with an optional colon, optional seconds, or a bare `Z`,
    but not CPython's fractional form: `'+010203.123456'` does not match, where CPython attaches an offset carrying
    those microseconds, which no Monty offset can hold (see [`timezone`](#timezone)).
- A `%z` offset whose minute and second separators disagree is rejected by both, but `'+0102:03'` (a colon before
    the seconds only) raises `ValueError: Inconsistent use of : in +0102:03` in Monty, where CPython leaks
    `ValueError: invalid literal for int() with base 10: ':0'`. The mirrored `'+01:0203'` matches CPython exactly.
- Input the format does not consume raises `ValueError: time data '...' does not match format '...'`,
    where CPython distinguishes trailing input with `ValueError: unconverted data remains: ...`.
- `utcnow()` (the deprecated class method) and `today()` are not
    implemented.
- `fromtimestamp()`, `fromordinal()` and `utcfromtimestamp()` are not
    implemented.
- `time()` and `timetz()` return a `time` whose `fold` is always 0, since
    `datetime` does not store the flag (above). `combine()` likewise
    discards the `fold` of the `time` it is given, where CPython carries it
    over.

Subclassing `datetime` is not possible, since there is no class inheritance
(see [classes.md](classes.md)).

`datetime.replace()`, `date.replace()` and `time.replace()` accept **only
keyword arguments** in Monty. CPython accepts positional args too
(`d.replace(2025)` is valid in CPython 3.14). Calling with positionals
in Monty raises `TypeError: replace expected at most 0 arguments, got N`.

## Reading the clock

The [session clock](../security.md#the-clock) has separate `datetime` and `timezone` settings.
The `time` module's wall clocks share the `datetime` source, and its conversion functions the `timezone`
(see [time.md](time.md)).

`datetime`:

- A fixed instant never advances: repeated `datetime.now()` calls are equal and elapsed-time calculations stay zero.
    Bindings also set `timezone` unless supplied explicitly: naive Python datetimes and JavaScript dates select UTC;
    aware Python datetimes supply their offset and name.
    A Rust fixed instant outside years 1–9999 raises `OverflowError: date value out of range` from all three clock calls.
- `'call_host'` delegates to the pool's `os=` handler (`OSAccess.date_today()` or `datetime_now()` in Python).
    Unanswered calls raise `RuntimeError: 'date.today' is not supported in this environment` (`datetime.now` likewise).
    Non-suspending Rust execution instead raises `NotImplementedError`.
    `run` and `feed_run` report `OS function 'datetime.now' not implemented with standard execution`;
    `call_function` reports
    `MontyRepl::call_function: OS function 'datetime.now' is not yet supported in this context`.

`timezone`:

- The default is UTC, not the host's zone, so a naive `datetime.now()` is the UTC wall clock and `astimezone()`
    attaches `timezone(timedelta(0), 'UTC')`.
- A fixed zone is a UTC offset with an optional name and no DST rules.
    `astimezone()`, `%Z` and `time.tzname` report the name (`UTC±HH:MM` when there is none).
- An IANA name (`'Europe/London'`) is resolved in the worker against its tz database: the OS copy under `TZDIR` or
    `/usr/share/zoneinfo`, or the copy bundled into the binary when the worker can read none (Windows, and the wasm
    worker).
    The offset and abbreviation then follow the instant, so the results depend on that database's version, as
    CPython's do on the host's.
    An unknown name is refused when the session is checked out.
    `time.timezone`, `time.altzone`, `time.daylight` and `time.tzname` are computed at import and need the clock's year, so a named zone under `datetime='call_host'` leaves them absent;
    see [time.md](time.md#zone-constants).

## `time`

Constructor: `time(hour=0, minute=0, second=0, microsecond=0, tzinfo=None, *, fold=0)`.
Attributes: `hour`, `minute`, `second`, `microsecond`, `tzinfo`, `fold`.
Methods: `isoformat(timespec='auto')`, `strftime`, `replace`,
`utcoffset`, `tzname`, `dst`.
Class methods: `fromisoformat(time_string)`. `strptime(string, format)`
— which CPython added in 3.14 — is not implemented and raises
`AttributeError`. `datetime.strptime` is not a workaround for a time-only
format: it requires the string to carry a date, where CPython defaults the
missing one to 1900-01-01.

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

Arithmetic (`+`, `-`, `*`, `/`, `//`, `%`, `divmod`, comparisons) works
between `timedelta`s and between `datetime`/`date` and `timedelta`.

## `timezone`

Constructor: `timezone(offset, name=None)` where `offset` is a
`timedelta`.
Attributes: none.
Methods: `utcoffset(dt)`, `tzname(dt)`, `dst(dt)`. The `dt` argument is
validated and then ignored, since the offset is the same at every
instant.

`fromutc(dt)` is not implemented, and neither is the abstract `tzinfo`
base class. Only fixed offsets can be constructed, so on `timezone`,
`datetime` and `time` alike, `utcoffset()` is constant over time and
`dst()` is always `None`.

An offset is a whole number of seconds: `timezone(timedelta(seconds=1, microseconds=1))` raises
`ValueError: offset must be a timedelta representing a whole number of seconds`, CPython's own message before 3.7,
where CPython now accepts it.

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

A directive that chrono *recognises* but can't render for the given value
raises `ValueError: Invalid format string` where CPython hands it to the C
library: `%+` (chrono's RFC 3339 form) and `%#z` (chrono's hour-only offset)
both need an offset the naive components lack, so they raise even on an aware
value's naive components. The other flagged forms of `%z` (`%-z`, `%_z`,
`%Ez`, `%Oz`) are unrecognised and pass through verbatim, where macOS CPython
renders them as an empty string.

`%z`, `%:z` and `%Z` are filled from `utcoffset()` and `tzname()` as in
CPython: empty for a naive `date`, `datetime` or `time`, and `'+0200'`,
`'+02:00'` and the zone name for an aware value. The name of an unnamed zone
is `UTC±HH:MM`, and a sub-minute offset renders its seconds (`'+023015'`).

f-strings and `str.format()` format `date`, `datetime` and `time` values through
`strftime`, matching CPython's `__format__`: `f'{dt:%Y-%m-%d}'` and
`'{:%Y-%m-%d}'.format(dt)` are equivalent to `dt.strftime('%Y-%m-%d')`, and
an empty spec uses `str(dt)`. One edge-case divergence remains for a literal
f-string spec that is also a valid format mini-language spec (e.g.
`f'{dt:>10}'` or a lone `f'{dt:%}'`): Monty applies generic string formatting,
where CPython treats the entire spec as a `strftime` string. Dynamically built
f-string specs and `str.format()` specs are handed to `strftime`.
