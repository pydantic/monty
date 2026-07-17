# call-external
# External calls in comparison expressions

# External call on left side of comparison
result = add_ints(1, 2) == 3
assert result == True

# External call on right side
result = 3 == add_ints(1, 2)
assert result == True

# Both sides external calls
result = add_ints(1, 2) == add_ints(2, 1)
assert result == True

# Less than
result = add_ints(1, 1) < add_ints(2, 2)
assert result == True

# Greater than
result = add_ints(5, 5) > add_ints(2, 2)
assert result == True

# Not equal
result = add_ints(1, 2) != add_ints(3, 4)
assert result == True
