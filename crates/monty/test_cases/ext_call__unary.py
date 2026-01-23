# call-external
# External calls in unary expressions

# Negation of external call result
result = -add_ints(3, 4)
assert result == -7, 'negation of ext call'

# Not of external call
result = not return_value(False)
assert result == True, 'not of ext call returning False'

result = not return_value(True)
assert result == False, 'not of ext call returning True'
