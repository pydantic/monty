# /// monty
# xfail = ['monty']
# ///
# Subscript deletion is unsupported; see limitations/builtins.md.
data = bytearray(b'abc')
del data[1]
assert data == b'ac'
