# Tests that values of different types are returned correctly
# Also tests identity operators with singletons

# === Boolean values ===
assert repr(False) == 'False'
assert repr(True) == 'True'

# === None value ===
assert repr(None) == 'None'

# === Ellipsis value ===
assert repr(...) == 'Ellipsis'

# === Ellipsis identity ===
assert (... is ...) == True
assert (None is ...) == False

# === Type checks against None ===
assert (False is None) == False
assert (True is None) == False
assert (None is None) == True
assert (42 is None) == False
assert (3.14 is None) == False
assert ([1, 2] is None) == False
assert ('hello' is None) == False
assert ((1, 2) is None) == False

# === Type checks against Ellipsis ===
assert (False is ...) == False
assert (True is ...) == False
assert (None is ...) == False
assert (42 is ...) == False
assert (3.14 is ...) == False
assert ([1, 2] is ...) == False
assert ('hello' is ...) == False
assert ((1, 2) is ...) == False
