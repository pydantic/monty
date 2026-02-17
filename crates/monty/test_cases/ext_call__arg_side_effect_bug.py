# call-external
# BUG: Side effects in arguments are duplicated when external call suspends
#
# When an external call is in one argument position and there's a side effect
# in another argument position, the side effect may be executed multiple times
# because argument evaluation is repeated during resumption.

call_count = 0


def side_effect(val):
    global call_count
    call_count += 1
    return val


# Side effect before external call in args
# Expected: side_effect runs once, result is 10 + 3 = 13
call_count = 0
result = add_ints(side_effect(10), add_ints(1, 2))
assert result == 13, 'ext call after side effect'
assert call_count == 1, 'side effect should happen only once (before ext)'
