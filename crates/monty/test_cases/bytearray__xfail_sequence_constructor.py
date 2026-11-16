# /// monty
# xfail = ['monty']
# ///
# The legacy __getitem__ iteration fallback is unsupported; see limitations/classes.md.
class Sequence:
    def __getitem__(self, index):
        return (1, 2, 3)[index]


assert bytearray(Sequence()) == b'\x01\x02\x03'
