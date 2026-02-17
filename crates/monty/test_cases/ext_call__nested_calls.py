# call-external
# External calls in nested call expressions

# Nested external calls - inner first
result = add_ints(add_ints(1, 2), 3)
assert result == 6, 'nested ext calls'

# Triple nested
result = add_ints(add_ints(add_ints(1, 1), 2), 3)
assert result == 7, 'triple nested ext calls'

# Two separate nested calls
result = add_ints(add_ints(1, 2), add_ints(3, 4))
assert result == 10, 'two nested ext calls in args'
