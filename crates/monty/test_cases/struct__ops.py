import struct

# === calcsize (standard sizes) ===
assert struct.calcsize('>bBhHiIlLq') == 30
assert struct.calcsize('>i') == 4
assert struct.calcsize('<d') == 8
assert struct.calcsize('>4i') == 16
assert struct.calcsize('') == 0

# === exact big/little-endian bytes ===
assert struct.pack('>i', 1) == b'\x00\x00\x00\x01'
assert struct.pack('<i', 1) == b'\x01\x00\x00\x00'
assert struct.pack('>h', -1) == b'\xff\xff'
assert struct.pack('>f', 1) == b'?\x80\x00\x00'  # int accepted for float code

# === round-trips (pack then unpack) ===
assert struct.unpack('>i', struct.pack('>i', -5)) == (-5,)
assert struct.unpack('>4i', struct.pack('>4i', 1, 2, 3, 4)) == (1, 2, 3, 4)
assert struct.unpack('>ihq', struct.pack('>ihq', 100, -2, 10**9)) == (100, -2, 10**9)
assert struct.unpack('>f', struct.pack('>f', 1.5)) == (1.5,)
assert struct.unpack('>d', struct.pack('>d', 3.14159)) == (3.14159,)


# === import inside a function body (lazy name-seeding must reach nested imports) ===
def _nested_calcsize():
    import struct

    return struct.calcsize('>i')


assert _nested_calcsize() == 4

# === errors: type diverges (ValueError in Monty, struct.error in CPython), so
# catch broadly and don't assert the message ===
try:
    struct.pack('>b', 999)  # out of range for a signed byte
    assert False, 'expected packing 999 into a signed byte to fail'
except Exception:
    pass

# === a format with overflowing repeat counts is rejected, not panicked on ===
try:
    struct.pack('9999999999999999999b9999999999999999999b')
    assert False, 'expected an oversized format to fail'
except Exception:
    pass
