# `math` module

Wide coverage; behaviour matches CPython 3.14 for the implemented set.

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

**Constants**: `pi`, `e`, `tau`, `inf`, `nan`.

## Not implemented

`fsum`, `prod`, `hypot`, `dist`, `sumprod`, `nan` from arbitrary
payloads.

## Behavioural notes

- `math.asin` / `math.acos` reject inputs outside `[-1, 1]` with
  `ValueError: "expected a number in range from -1 up to 1, got <x>"`.
  CPython uses `"math domain error"` — Monty's message is more specific.
- Domain errors (e.g. `log(-1)`) raise `ValueError: "math domain error"`
  matching CPython.
- Overflow (finite input → infinite result) raises `OverflowError: "math
  range error"` matching CPython.
- `math.gamma` rejects non-positive integers (poles) with `ValueError`.
