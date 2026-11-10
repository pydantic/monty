# `statistics` module

`mean`, `fmean`, `median`, `median_low`, `median_high`, `mode`, `multimode`, `pstdev`, `pvariance`, `stdev`,
`variance`, `quantiles` and `StatisticsError` are available.

- **`NormalDist`, `kde`, `kde_random`, `geometric_mean`, `harmonic_mean`, `median_grouped`, `covariance`,
    `correlation` and `linear_regression` are not provided.** They are missing from the module namespace, so they fail
    type checking as well as raising `AttributeError` at runtime.
- **`Decimal` and `Fraction` data are not supported** by `mean`, `variance`, `pvariance`, `stdev` and `pstdev`.
    Monty has no `fractions` module, and these functions raise
    `TypeError: can't convert type '<type>' to numerator/denominator`, the error CPython gives for a type with no
    `as_integer_ratio()`. `median_low`, `median_high` and `mode` work on any orderable (or hashable) value, as in
    CPython; `median` and `quantiles` also need data that can be added and divided.
- **`mode()` and `multimode()` of unhashable data raise**
    `TypeError: cannot use 'list' as a dict key (unhashable type: 'list')` where CPython says only
    `unhashable type: 'list'`.
- **`StatisticsError` reprs as `statistics.StatisticsError(...)`**, as the other dotted exception classes do (see
    [exceptions.md](exceptions.md)).
- **`quantiles()` with an `n` too large to allocate raises `MemoryError`** before computing any cut point, where
    CPython appends cut points until it runs out of memory.
- **The module has no `__all__`**, and its functions carry no attributes, as on every built-in module
    (see [modules.md](modules.md)).
