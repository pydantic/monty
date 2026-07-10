# call-external
# External calls in ternary expressions (if/else expressions)

# External call in true branch
result = add_ints(1, 2) if True else add_ints(10, 20)
assert result == 3

# External call in false branch
result = add_ints(1, 2) if False else add_ints(10, 20)
assert result == 30

# External call in condition
result = 'yes' if return_value(True) else 'no'
assert result == 'yes'

result = 'yes' if return_value(False) else 'no'
assert result == 'no'

# External calls in both branches
result = add_ints(1, 2) if return_value(True) else add_ints(10, 20)
assert result == 3

result = add_ints(1, 2) if return_value(False) else add_ints(10, 20)
assert result == 30

# Nested ternary with external calls
result = add_ints(1, 1) if return_value(True) else (add_ints(2, 2) if return_value(False) else add_ints(3, 3))
assert result == 2
