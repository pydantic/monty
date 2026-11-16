# /// monty
# xfail = ['monty']
# ///
# CPython escapes single quotes even inside a double-quoted bytearray repr.
# Adapted from Lib/test/test_bytes.py, AssortedBytesTest.test_bytearray_repr.
assert repr(bytearray(b"'")) == 'bytearray(b"\\\'")'
