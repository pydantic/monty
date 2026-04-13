# call-external
# BUG: This test hangs (infinite loop) - external calls in boolean expressions
# with side effects cause incorrect behavior.

call_count = 0


def side_effect(val):
    global call_count
    call_count += 1
    return val


# This specific pattern causes a hang in Monty
result = return_value(True) and return_value(side_effect(42))
assert result == 42, 'and: second runs when first truthy'
assert call_count == 1, 'and: side effect runs when first is truthy'
