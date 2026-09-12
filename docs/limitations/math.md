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
- `math.log`, `log2` and `log10` of an int beyond the float range compute
    `log(m) + log(2) * e` from the int's leading bits, as CPython does. Monty
    never fuses that multiply-add, so the result can differ from CPython by one
    unit in the last place on builds whose C compiler emits a fused
    multiply-add (CPython on Apple Silicon does; Linux x86-64 and Windows
    builds do not). For example `math.log(3**700)` is `769.0286020676768` in
    Monty and `769.0286020676767` in CPython on macOS arm64.
