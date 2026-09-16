# Tests reference counting on the divmod() operand paths.
#
# builtin_divmod guards both operands with defer_drop! and hands them to
# Value::py_divmod, which may allocate a result tuple, dispatch through the
# heap to LongInt/timedelta, or raise. Every one of those exits has to leave
# the operands' counts where it found them.

import datetime

big = 2**70
neg_big = -(2**70)
delta = datetime.timedelta(hours=3)
divisor = datetime.timedelta(hours=-2)
zero_delta = datetime.timedelta(0)

# Success paths that allocate a tuple: immediate, bigint direct and reflected,
# mixed float, and the timedelta pair.
assert divmod(7, 2) == (3, 1)
assert divmod(big, 3)[1] == 1
assert divmod(3, big) == (0, 3)
assert divmod(big, big) == (1, 0)
assert divmod(neg_big, big) == (-1, 0)
assert divmod(big, 2.0)[1] == 0.0
assert divmod(2.0, big)[0] == 0.0
assert divmod(delta, divisor) == (-2, datetime.timedelta(hours=-1))

# Error paths, which must drop both operands without building a result:
# unsupported pair, zero divisor on each of the int, bigint and timedelta arms.
for compute in [
    lambda: divmod(big, 'x'),
    lambda: divmod('x', big),
    lambda: divmod(delta, big),
    lambda: divmod(big, delta),
    lambda: divmod(big, 0),
    lambda: divmod(big, False),
    lambda: divmod(3, big - big),
    lambda: divmod(big, 0.0),
    lambda: divmod(delta, zero_delta),
]:
    try:
        compute()
        assert False, 'expected divmod to fail'
    except (TypeError, ZeroDivisionError):
        pass

# Every name above is held by exactly one variable; `datetime` is the module.
# The trailing expression adds a second reference to `big`. A leaked operand
# would show up as an unreachable heap object before these counts are compared.
big
# ref-counts={'datetime': 1, 'delta': 1, 'big': 2, 'neg_big': 1, 'zero_delta': 1, 'divisor': 1}
