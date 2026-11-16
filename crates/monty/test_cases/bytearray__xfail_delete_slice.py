# /// monty
# xfail = ['monty']
# ///
# Slice deletion is unsupported; see limitations/builtins.md.
data = bytearray(b'abcde')
del data[1:4]
assert data == b'ae'
