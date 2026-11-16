# /// monty
# xfail = ['monty']
# ///
# Builtin bytearrays do not expose __class__; see limitations/builtins.md.
assert bytearray().__class__ is bytearray
