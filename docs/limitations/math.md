# `math` module

Behaviour matches CPython 3.14 for the implemented set, apart from the notes
below.

## Implemented

**Rounding**: `floor`, `ceil`, `trunc`.
**Roots / powers**: `sqrt`, `isqrt`, `cbrt`, `pow`, `exp`, `exp2`, `expm1`.
**Logarithms**: `log`, `log2`, `log10`, `log1p`.
**Trig**: `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`.
**Hyperbolic**: `sinh`, `cosh`, `tanh`, `asinh`, `acosh`, `atanh`.
**Angles**: `degrees`, `radians`.
**Float properties**: `fabs`, `isnan`, `isinf`, `isfinite`, `copysign`,
`isclose`, `nextafter`, `ulp`.
**Integer math**: `factorial`, `gcd`, `lcm`, `comb`, `perm`.
**Modular**: `fmod`, `remainder`, `modf`, `frexp`, `ldexp`.
**Special**: `gamma`, `lgamma`, `erf`, `erfc`.
**Summation / products**: `hypot`, `dist`, `fsum`, `prod`, `sumprod`, `fma`.

**Constants**: `pi`, `e`, `tau`, `inf`, `nan`.

## Behavioural notes

- Real-number arguments accept floats, integers of any size, and booleans, but do not call user-defined `__float__` or
    `__index__` methods.
- `factorial`, `comb`, and `perm` raise `OverflowError` for results exceeding the signed 64-bit integer range.
    `prod` and integer-only `sumprod` support arbitrary-size integer results.
