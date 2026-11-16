# /// monty
# xfail = ['monty']
# ///
# Builtin methods currently support direct calls only; see limitations/builtins.md.
data = bytearray(b'a')
append = data.append
append(98)
assert data == b'ab'
