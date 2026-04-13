# call-external
# BUG: External calls in recursive functions produce wrong results
# Recursion with external calls doesn't compute the correct value


def sum_with_ext(n):
    if n <= 0:
        return 0
    return add_ints(n, sum_with_ext(n - 1))


# sum_with_ext(3) should compute:
#   add_ints(3, sum_with_ext(2))
#   add_ints(3, add_ints(2, sum_with_ext(1)))
#   add_ints(3, add_ints(2, add_ints(1, sum_with_ext(0))))
#   add_ints(3, add_ints(2, add_ints(1, 0)))
#   = 3 + 2 + 1 = 6
result = sum_with_ext(3)
assert result == 6, 'recursive ext call: 1+2+3=6'
