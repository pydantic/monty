# === Boolean 'and' operator ===
# returns first falsy value, or last value if all truthy
assert (5 and 3) == 3
assert (0 and 3) == 0
assert (1 and 2 and 3) == 3

# === Boolean 'or' operator ===
# returns first truthy value, or last value if all falsy
assert (5 or 3) == 5
assert (0 or 3) == 3
assert (0 or 0 or 3) == 3

# === Boolean 'not' operator ===
assert (not 5) == False
assert (not 0) == True
assert (not None) == True

# === Complex boolean expressions ===
assert ((1 and 2) or (3 and 0)) == 2
assert (not (0 and 1)) == True
