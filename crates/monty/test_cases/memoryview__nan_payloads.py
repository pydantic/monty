def little_bytes(value, size):
    return bytes(bytearray([(value >> (8 * index)) & 255 for index in range(size)]))


# Binary16 NaNs preserve their sign, payload, and signaling bit when widened to binary64.
for sign in [0, 0x8000]:
    for fraction in range(1, 1024):
        half_bits = sign | 0x7C00 | fraction
        raw = little_bytes(half_bits, 2)
        view = memoryview(bytearray(raw)).cast('e')
        value = view[0]
        assert value != value
        wide = bytearray(8)
        memoryview(wide).cast('d')[0] = value
        expected_bits = ((sign >> 15) << 63) | (0x7FF << 52) | (fraction << 42)
        assert wide == little_bytes(expected_bits, 8)
        view[0] = value
        assert view.tobytes() == raw

# If all NaN payload bits would be truncated, packing produces a quiet NaN, not infinity.
for bits, expected in [
    (0x7FF0000000000001, 0x7E00),
    (0xFFF0000000000001, 0xFE00),
    (0x7FF4000000000000, 0x7D00),
    (0x7FFB280000000000, 0x7ECA),
    (0x7FFFFFFFFFFFFFFF, 0x7FFF),
]:
    value = memoryview(little_bytes(bits, 8)).cast('d')[0]
    target = bytearray(2)
    memoryview(target).cast('e')[0] = value
    assert target == little_bytes(expected, 2)
