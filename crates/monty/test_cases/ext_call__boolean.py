# call-external
# External calls in boolean short-circuit expressions

# === Basic boolean operations ===
result = return_value(True) and return_value(42)
assert result == 42, 'ext call in and (both run)'

result = return_value(False) and return_value(42)
assert result == False, 'ext call in and (short circuit)'

result = return_value(0) or return_value(42)
assert result == 42, 'ext call in or (both run)'

result = return_value(99) or return_value(42)
assert result == 99, 'ext call in or (short circuit)'


# === Chained boolean with external calls ===
result = return_value(True) and return_value(True) and return_value(42)
assert result == 42, 'chained and all truthy'

result = return_value(True) and return_value(False) and return_value(42)
assert result == False, 'chained and with false in middle'

result = return_value(0) or return_value(0) or return_value(42)
assert result == 42, 'chained or all falsy except last'

result = return_value(0) or return_value(99) or return_value(42)
assert result == 99, 'chained or with truthy in middle'


# === Mixed and/or ===
result = return_value(True) and return_value(0) or return_value(42)
assert result == 42, 'and then or'

result = return_value(0) or return_value(True) and return_value(42)
assert result == 42, 'or then and'
