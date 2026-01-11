# xfail=monty
# Monty's list += only supports other lists, not arbitrary iterables.
# CPython's list += calls extend() which requires an iterable.
x = [1]
x += 2
# Raise=TypeError("'int' object is not iterable")
