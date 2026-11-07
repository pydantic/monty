# `statistics` module

`mean`, `fmean`, `median`, `median_low`, `median_high`, `mode`, `multimode`, `pstdev`, `pvariance`, `stdev`,
`variance`, `quantiles` and `StatisticsError` are available, with CPython's signatures, results and error messages.

`mean`, `variance`, `pvariance`, `stdev` and `pstdev` sum `int`, `bool` and `float` data exactly and round once at the
end, as CPython does with `fractions.Fraction`, so results are bit-for-bit identical, including the `int` result for
`int` data whose mean or variance is a whole number. Any iterable works, not only a list.

- **`NormalDist`, `kde`, `kde_random`, `geometric_mean`, `harmonic_mean`, `median_grouped`, `covariance`,
    `correlation` and `linear_regression` are not provided.** They are missing from the module namespace, so they fail
    type checking as well as raising `AttributeError` at runtime.
- **`Decimal` and `Fraction` data are not supported** by `mean`, `variance`, `pvariance`, `stdev` and `pstdev`.
    Monty has no `fractions` module, and these functions raise
    `TypeError: can't convert type '<type>' to numerator/denominator`, the error CPython gives for a type with no
    `as_integer_ratio()`. `median`, `median_low`, `median_high`, `quantiles` and `mode` work on any orderable (or
    hashable) value, as in CPython.
- **`mode()` and `multimode()` of unhashable data raise**
    `TypeError: cannot use 'list' as a dict key (unhashable type: 'list')` where CPython says only
    `unhashable type: 'list'`.
- **`StatisticsError` is a `ValueError` subclass**, and its `repr` is `statistics.StatisticsError(...)`, as for the
    other module-specific exceptions such as `json.JSONDecodeError`.
- **`quantiles()` needs an `int` for `n`**; a `float` raises `TypeError`.
- **The module has no `__all__`**, and its functions carry no attributes, as on every built-in module
    (see [modules.md](modules.md)).
