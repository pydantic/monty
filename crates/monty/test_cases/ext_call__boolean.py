# call-external
# External calls in boolean short-circuit expressions

# === Basic boolean operations ===
result = return_value(True) and return_value(42)
assert result == 42

result = return_value(False) and return_value(42)
assert result == False

result = return_value(0) or return_value(42)
assert result == 42

result = return_value(99) or return_value(42)
assert result == 99


# === Chained boolean with external calls ===
result = return_value(True) and return_value(True) and return_value(42)
assert result == 42

result = return_value(True) and return_value(False) and return_value(42)
assert result == False

result = return_value(0) or return_value(0) or return_value(42)
assert result == 42

result = return_value(0) or return_value(99) or return_value(42)
assert result == 99


# === Mixed and/or ===
result = return_value(True) and return_value(0) or return_value(42)
assert result == 42

result = return_value(0) or return_value(True) and return_value(42)
assert result == 42
