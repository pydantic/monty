# `zoneinfo` module

`ZoneInfo` and `available_timezones()` are available. A `ZoneInfo` resolves its key against the tz database the
worker carries, the same one the session's `timezone` setting uses (see [datetime.md](datetime.md#reading-the-clock)),
so offsets and abbreviations depend on that database's version, as CPython's depend on the host's.

- **`ZoneInfoNotFoundError` is `KeyError` itself.** Monty's exception types are a fixed set, so the module binds the
    name to `KeyError`; an unknown key raises `KeyError('No time zone found with key ...')`, which
    `except zoneinfo.ZoneInfoNotFoundError` catches along with every other `KeyError`.
- **There is no instance cache.** `ZoneInfo('UTC') is ZoneInfo('UTC')` is `False` where CPython caches instances;
    equality compares keys instead, so `ZoneInfo('UTC') == ZoneInfo.no_cache('UTC')` is `True` where CPython's
    identity-based equality says `False`. `ZoneInfo.clear_cache()` does nothing.
- **Keys are case-sensitive.** `ZoneInfo('europe/london')` raises; CPython accepts it on a case-insensitive
    filesystem.
- **`ZoneInfo.from_file()`, `TZPATH`, `reset_tzpath()` and `InvalidTZPathWarning` are absent.**
- **`fold` is not read.** A `datetime` in a repeated hour takes the first occurrence
    (`datetime(2024, 10, 27, 1, 30, fold=1, tzinfo=ZoneInfo('Europe/London')).utcoffset()` is one hour, not zero).
    A wall clock in a DST gap takes the offset from before the transition, as CPython's `fold=0` does.
- **`datetime.dst()` approximates the saving as the difference from the zone's standard offset that year**, read as
    `time.timezone` reads it. Zones whose standard offset changed within a year may report a different value.
- **`time` objects accept only fixed offsets.** `time(12, tzinfo=ZoneInfo('UTC'))` raises `TypeError`, where CPython
    accepts it with `utcoffset()` returning `None`; `datetime.timetz()` on a `ZoneInfo` value attaches a `timezone`
    carrying the offset and abbreviation in force at that instant.
- **`datetime.now(ZoneInfo(...))` needs the session clock.** Under a `call_host` clock it raises
    `NotImplementedError`, since the offset depends on the instant only the host knows.
- **A `ZoneInfo` cannot cross the host boundary.** An aware `datetime` carrying one is exported with the fixed
    offset and abbreviation in force at that instant.
- **Error messages name the type with its module** (`'zoneinfo.ZoneInfo' object has no attribute 'x'`), where
    CPython writes `ZoneInfo`.
