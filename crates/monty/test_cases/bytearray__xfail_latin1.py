# /// monty
# xfail = ['monty']
# ///
# Legacy codecs are unsupported; see limitations/encoding.md.
assert bytearray('é', 'latin-1') == b'\xe9'
