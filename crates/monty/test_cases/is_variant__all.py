# Tests that values of different types are returned correctly
# Also tests identity operators with singletons

# === Boolean values ===
assert repr(False) == 'False'
assert repr(True) == 'True'

# === None value ===
assert repr(None) == 'None'

# === Ellipsis value ===
assert repr(...) == 'Ellipsis'
assert repr(Ellipsis) == 'Ellipsis'

# === NotImplemented value ===
assert repr(NotImplemented) == 'NotImplemented'

# === Ellipsis identity ===
assert (... is ...) == True
assert (Ellipsis is ...) == True
assert (None is ...) == False

# === NotImplemented identity ===
assert (NotImplemented is NotImplemented) == True
assert (None is NotImplemented) == False
assert (... is NotImplemented) == False

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

# === Type checks against NotImplemented ===
assert (False is NotImplemented) == False
assert (True is NotImplemented) == False
assert (None is NotImplemented) == False
assert (42 is NotImplemented) == False
assert (3.14 is NotImplemented) == False
assert ([1, 2] is NotImplemented) == False
assert ('hello' is NotImplemented) == False
assert ((1, 2) is NotImplemented) == False
