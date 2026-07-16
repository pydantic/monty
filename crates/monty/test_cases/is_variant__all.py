# Tests that values of different types are returned correctly
# Also tests identity operators with singletons

# === Boolean values ===
assert repr(False) == 'False', 'False repr'
assert repr(True) == 'True', 'True repr'

# === None value ===
assert repr(None) == 'None', 'None repr'

# === Ellipsis value ===
assert repr(...) == 'Ellipsis', 'Ellipsis repr'
assert repr(Ellipsis) == 'Ellipsis', 'Ellipsis builtin repr'

# === NotImplemented value ===
assert repr(NotImplemented) == 'NotImplemented', 'NotImplemented repr'

# === Ellipsis identity ===
assert (... is ...) == True, 'ellipsis is ellipsis'
assert (Ellipsis is ...) == True, 'Ellipsis builtin is ellipsis literal'
assert (None is ...) == False, 'none is not ellipsis'

# === NotImplemented identity ===
assert (NotImplemented is NotImplemented) == True, 'NotImplemented is NotImplemented'
assert (None is NotImplemented) == False, 'none is not NotImplemented'
assert (... is NotImplemented) == False, 'ellipsis is not NotImplemented'

# === Type checks against None ===
assert (False is None) == False, 'False is not None'
assert (True is None) == False, 'True is not None'
assert (None is None) == True, 'None is None'
assert (42 is None) == False, 'int is not None'
assert (3.14 is None) == False, 'float is not None'
assert ([1, 2] is None) == False, 'list is not None'
assert ('hello' is None) == False, 'str is not None'
assert ((1, 2) is None) == False, 'tuple is not None'

# === Type checks against Ellipsis ===
assert (False is ...) == False, 'False is not Ellipsis'
assert (True is ...) == False, 'True is not Ellipsis'
assert (None is ...) == False, 'None is not Ellipsis'
assert (42 is ...) == False, 'int is not Ellipsis'
assert (3.14 is ...) == False, 'float is not Ellipsis'
assert ([1, 2] is ...) == False, 'list is not Ellipsis'
assert ('hello' is ...) == False, 'str is not Ellipsis'
assert ((1, 2) is ...) == False, 'tuple is not Ellipsis'

# === Type checks against NotImplemented ===
assert (False is NotImplemented) == False, 'False is not NotImplemented'
assert (True is NotImplemented) == False, 'True is not NotImplemented'
assert (None is NotImplemented) == False, 'None is not NotImplemented'
assert (42 is NotImplemented) == False, 'int is not NotImplemented'
assert (3.14 is NotImplemented) == False, 'float is not NotImplemented'
assert ([1, 2] is NotImplemented) == False, 'list is not NotImplemented'
assert ('hello' is NotImplemented) == False, 'str is not NotImplemented'
assert ((1, 2) is NotImplemented) == False, 'tuple is not NotImplemented'
