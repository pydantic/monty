# call-external
# External calls in unary expressions

# Negation of external call result
result = -add_ints(3, 4)
assert result == -7

# Not of external call
result = not return_value(False)
assert result == True

result = not return_value(True)
assert result == False
