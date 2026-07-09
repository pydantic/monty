from decimal import Decimal as D
import decimal


def expect(fn, exc_type, message):
    try:
        fn()
        assert False, f'expected {exc_type.__name__}'
    except exc_type as exc:
        assert str(exc) == message, f'{message!r} != {str(exc)!r}'


# === Division by zero ===
# x / 0 (x != 0) -> DivisionByZero
expect(lambda: D(1) / D(0), decimal.DivisionByZero, "[<class 'decimal.DivisionByZero'>]")
expect(lambda: D(1) // D(0), decimal.DivisionByZero, "[<class 'decimal.DivisionByZero'>]")
expect(lambda: D(1) / 0, decimal.DivisionByZero, "[<class 'decimal.DivisionByZero'>]")
expect(lambda: D('2.5') / D(0), decimal.DivisionByZero, "[<class 'decimal.DivisionByZero'>]")

# 0 / 0 -> InvalidOperation (DivisionUndefined)
expect(lambda: D(0) / D(0), decimal.InvalidOperation, "[<class 'decimal.DivisionUndefined'>]")
expect(lambda: D(0) // D(0), decimal.InvalidOperation, "[<class 'decimal.DivisionUndefined'>]")
expect(lambda: D(0) % D(0), decimal.InvalidOperation, "[<class 'decimal.DivisionUndefined'>]")

# x % 0 (x != 0) -> InvalidOperation (plain)
expect(lambda: D(1) % D(0), decimal.InvalidOperation, "[<class 'decimal.InvalidOperation'>]")

# divmod(x, 0) reports both InvalidOperation and DivisionByZero
expect(
    lambda: divmod(D(1), D(0)),
    decimal.InvalidOperation,
    "[<class 'decimal.InvalidOperation'>, <class 'decimal.DivisionByZero'>]",
)
expect(lambda: divmod(D(0), D(0)), decimal.InvalidOperation, "[<class 'decimal.DivisionUndefined'>]")

# === Invalid infinity operations ===
expect(lambda: D('inf') - D('inf'), decimal.InvalidOperation, "[<class 'decimal.InvalidOperation'>]")
expect(lambda: D('-inf') + D('inf'), decimal.InvalidOperation, "[<class 'decimal.InvalidOperation'>]")
expect(lambda: D('inf') * D(0), decimal.InvalidOperation, "[<class 'decimal.InvalidOperation'>]")
expect(lambda: D('inf') / D('inf'), decimal.InvalidOperation, "[<class 'decimal.InvalidOperation'>]")
expect(lambda: D('inf') % D(2), decimal.InvalidOperation, "[<class 'decimal.InvalidOperation'>]")
expect(lambda: D('inf') // D('inf'), decimal.InvalidOperation, "[<class 'decimal.InvalidOperation'>]")
expect(lambda: divmod(D('inf'), D(2)), decimal.InvalidOperation, "[<class 'decimal.InvalidOperation'>]")

# === Integer division whose quotient exceeds precision -> DivisionImpossible ===
# CPython refuses to materialise an integer quotient wider than `context.prec`
# (default 28 digits), so `//`, `%` and `divmod` all raise rather than rounding.
expect(lambda: D('1e40') // D(3), decimal.InvalidOperation, "[<class 'decimal.DivisionImpossible'>]")
expect(lambda: D('1e28') // D(1), decimal.InvalidOperation, "[<class 'decimal.DivisionImpossible'>]")
expect(lambda: D('1e40') % D(7), decimal.InvalidOperation, "[<class 'decimal.DivisionImpossible'>]")
expect(lambda: divmod(D('1e40'), D(3)), decimal.InvalidOperation, "[<class 'decimal.DivisionImpossible'>]")
# DivisionImpossible is catchable as InvalidOperation's parents.
expect(lambda: D('1e40') // D(3), decimal.DecimalException, "[<class 'decimal.DivisionImpossible'>]")
expect(lambda: D('1e40') // D(3), ArithmeticError, "[<class 'decimal.DivisionImpossible'>]")

# === Power errors ===
# Negative base to a non-integer power is undefined over the reals.
expect(lambda: D(-2) ** D('0.5'), decimal.InvalidOperation, "[<class 'decimal.InvalidOperation'>]")
expect(lambda: D(-4) ** D('0.5'), decimal.InvalidOperation, "[<class 'decimal.InvalidOperation'>]")
# A result whose magnitude exceeds the representable range overflows (rather than
# saturating to Infinity or — for an astronomically large exponent — crashing).
expect(lambda: D(2) ** D('1E1000'), decimal.Overflow, "[<class 'decimal.Overflow'>]")
expect(lambda: D(2) ** D('1E16384'), decimal.Overflow, "[<class 'decimal.Overflow'>]")

# === Hierarchy: DivisionByZero is catchable as ZeroDivisionError ===
try:
    D(1) / D(0)
    assert False, 'expected ZeroDivisionError'
except ZeroDivisionError:
    pass

# DivisionByZero and InvalidOperation are catchable as DecimalException and ArithmeticError
for fn in [lambda: D(1) / D(0), lambda: D(0) / D(0), lambda: D('inf') - D('inf')]:
    try:
        fn()
        assert False, 'expected DecimalException'
    except decimal.DecimalException:
        pass
for fn in [lambda: D(1) / D(0), lambda: D(0) / D(0)]:
    try:
        fn()
        assert False, 'expected ArithmeticError'
    except ArithmeticError:
        pass

# === Construction error is InvalidOperation (ConversionSyntax) ===
expect(lambda: D('not a number'), decimal.InvalidOperation, "[<class 'decimal.ConversionSyntax'>]")


# === Full signal exception taxonomy (multi-parent MRO) ===
# These signals are not raised by default (untrapped), but the classes are
# importable and the subclass relationships must match CPython. We exercise the
# `is_subclass_of` wiring by raising each and catching via every parent.
def caught_as(raise_type, catch_type):
    # Construct the signal *outside* the try so a constructor regression
    # surfaces as a test failure rather than being swallowed by the
    # `except BaseException` fallback (which would make the negative
    # assertions below pass vacuously).
    exc = raise_type('signal')
    try:
        raise exc
    except catch_type:
        return True
    except BaseException:
        return False


# Underflow ⊂ (Inexact, Rounded, Subnormal, DecimalException, ArithmeticError)
assert caught_as(decimal.Underflow, decimal.Inexact), 'Underflow is Inexact'
assert caught_as(decimal.Underflow, decimal.Rounded), 'Underflow is Rounded'
assert caught_as(decimal.Underflow, decimal.Subnormal), 'Underflow is Subnormal'
assert caught_as(decimal.Underflow, decimal.DecimalException), 'Underflow is DecimalException'
assert caught_as(decimal.Underflow, ArithmeticError), 'Underflow is ArithmeticError'
# Overflow ⊂ (Inexact, Rounded)
assert caught_as(decimal.Overflow, decimal.Inexact), 'Overflow is Inexact'
assert caught_as(decimal.Overflow, decimal.Rounded), 'Overflow is Rounded'
# Inexact / Rounded / Subnormal / Clamped ⊂ DecimalException ⊂ ArithmeticError
for leaf in [decimal.Inexact, decimal.Rounded, decimal.Subnormal, decimal.Clamped]:
    assert caught_as(leaf, decimal.DecimalException), f'{leaf.__name__} is DecimalException'
    assert caught_as(leaf, ArithmeticError), f'{leaf.__name__} is ArithmeticError'
# FloatOperation ⊂ (DecimalException, TypeError)
assert caught_as(decimal.FloatOperation, decimal.DecimalException), 'FloatOperation is DecimalException'
assert caught_as(decimal.FloatOperation, TypeError), 'FloatOperation is TypeError'
# Negative: a parent is not a subclass of its child
assert not caught_as(decimal.Inexact, decimal.Overflow), 'Inexact is not Overflow'
assert not caught_as(decimal.DecimalException, decimal.Inexact), 'DecimalException is not Inexact'

# === Finer InvalidOperation condition subtypes ===
# Importable and catchable as `InvalidOperation` (and `DivisionUndefined` as a
# `ZeroDivisionError`), matching CPython. Monty raises plain `InvalidOperation`,
# never these subtypes, so they participate in the hierarchy but are not raised.
for subtype in [
    decimal.ConversionSyntax,
    decimal.DivisionImpossible,
    decimal.DivisionUndefined,
    decimal.InvalidContext,
]:
    assert caught_as(subtype, decimal.InvalidOperation), f'{subtype.__name__} is InvalidOperation'
    assert caught_as(subtype, decimal.DecimalException), f'{subtype.__name__} is DecimalException'
    assert caught_as(subtype, ArithmeticError), f'{subtype.__name__} is ArithmeticError'
assert caught_as(decimal.DivisionUndefined, ZeroDivisionError), 'DivisionUndefined is ZeroDivisionError'
assert not caught_as(decimal.ConversionSyntax, decimal.DivisionImpossible), 'siblings are unrelated'
assert not caught_as(decimal.InvalidOperation, decimal.ConversionSyntax), 'parent is not its subtype'

# === Exponent literal bounds (C-module parity) ===
expect(lambda: D('1E+1000000000000000000'), decimal.InvalidOperation, "[<class 'decimal.InvalidOperation'>]")
expect(lambda: D('1E-2000000000000000000'), decimal.InvalidOperation, "[<class 'decimal.InvalidOperation'>]")

# === Signaling NaN operands raise InvalidOperation ===
expect(lambda: D('sNaN') + 1, decimal.InvalidOperation, "[<class 'decimal.InvalidOperation'>]")
expect(lambda: D(1) * D('-sNaN5'), decimal.InvalidOperation, "[<class 'decimal.InvalidOperation'>]")
expect(lambda: D('sNaN') < 1, decimal.InvalidOperation, "[<class 'decimal.InvalidOperation'>]")

# === quantize bounds ===
expect(lambda: D(1).quantize(D('inf')), decimal.InvalidOperation, "[<class 'decimal.InvalidOperation'>]")
expect(lambda: D('1e100').quantize(D('1e-100')), decimal.InvalidOperation, "[<class 'decimal.InvalidOperation'>]")

# === Tuple-form exponent bounds (same C-module limits as string literals) ===
expect(lambda: D((0, (5,), 9223372036854775807)), decimal.InvalidOperation, "[<class 'decimal.InvalidOperation'>]")
expect(lambda: D((0, (5,), -2 * 10**18)), decimal.InvalidOperation, "[<class 'decimal.InvalidOperation'>]")
assert str(D((0, (5,), 999999999999999999))) == '5E+999999999999999999', 'boundary exponent accepted'
expect(lambda: D((0, (5,), 10**30)), OverflowError, 'Python int too large to convert to C ssize_t')

# === round(Decimal, huge int) overflows like CPython's ssize_t conversion ===
expect(lambda: round(D('1.5'), 10**30), OverflowError, 'Python int too large to convert to C ssize_t')

# === copy_sign missing-argument wording (C-module parser) ===
expect(lambda: D(1).copy_sign(), TypeError, "function missing required argument 'other' (pos 1)")
