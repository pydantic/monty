# xfail=cpython
# CPython represents range parameters as arbitrary-precision integers, so a
# slice whose computed start/stop/step exceeds i64 simply produces a bigint
# range and succeeds. Monty's Range stores i64 and cannot represent those
# values, so we raise OverflowError instead — this divergence is intentional
# and acceptable.

try:
    _sliced = range(0, 2**63 - 1, 2)[:: 2**63 - 1]
    assert False, 'expected OverflowError from slice with overflowing step'
except OverflowError as e:
    assert str(e) == 'Python int too large to convert to C ssize_t', str(e)
