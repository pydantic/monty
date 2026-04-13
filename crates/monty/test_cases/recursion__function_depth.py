# Test that recursive function calls hit the recursion limit
# This uses Python function call recursion which both CPython and Monty limit


def recurse(n):
    if n == 0:
        return 0
    return recurse(n - 1) + 1


# This should raise RecursionError in both interpreters
recurse(2000)
# Raise=RecursionError('maximum recursion depth exceeded')
