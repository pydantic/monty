# /// monty
# xfail = ['monty']
# ///
# Persistent buffer exports are deferred to #1228; see limitations/builtins.md.
data = bytearray(b'a')
view = memoryview(data)
view[0] = 98
assert data == b'b'
