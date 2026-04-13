# call-external
# External calls in comparison expressions

# External call on left side of comparison
result = add_ints(1, 2) == 3
assert result == True, 'ext call == literal'

# External call on right side
result = 3 == add_ints(1, 2)
assert result == True, 'literal == ext call'

# Both sides external calls
result = add_ints(1, 2) == add_ints(2, 1)
assert result == True, 'ext call == ext call'

# Less than
result = add_ints(1, 1) < add_ints(2, 2)
assert result == True, 'ext call < ext call'

# Greater than
result = add_ints(5, 5) > add_ints(2, 2)
assert result == True, 'ext call > ext call'

# Not equal
result = add_ints(1, 2) != add_ints(3, 4)
assert result == True, 'ext call != ext call'
