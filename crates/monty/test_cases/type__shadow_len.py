# Builtin function 'len' can be shadowed by assignment
len = 'shadowed'
assert len == 'shadowed', 'len shadowed by assignment'
