# call-external
# External function calls in deep call stacks (function calling function).
# Tests that the outer function receives the return value correctly when
# the inner function makes an external call.


def depth1(n):
    return add_ints(n, 1)


def depth2(n):
    return depth1(n) + 10


result = depth2(5)
# depth2(5) should be: depth1(5) + 10 = 6 + 10 = 16

assert result == 16, f'ext call through 2 levels of functions {result=}'
