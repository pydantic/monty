# Integer storage can change while aliases and iterators keep the same list.
items = list(range(6))
alias = items
iterator = iter(items)
assert next(iterator) == 0
items.append(True)
assert alias is items
assert list(iterator) == [1, 2, 3, 4, 5, True]
assert type(alias[-1]) is bool

items.clear()
items.extend(range(8))
items[1::2] = [10, 11, 12, 13]
items[2:4] = [20, 21, 22]
assert items == [0, 10, 20, 21, 22, 4, 12, 6, 13]
assert items[::-2] == [13, 12, 22, 20, 0]
items.reverse()
items.insert(1, 99)
assert items.pop(1) == 99

copy = items.copy()
assert copy == items and copy is not items
assert items + copy == items * 2
copy += copy
assert copy == items * 2
copy *= 3
assert copy == items * 6
assert [1, 2, 3] * 1000 == [i % 3 + 1 for i in range(3000)]

# Reentrant sort sees the temporary empty list even when the original is packed.
seen = []


def sort_key(item):
    seen.append(len(items))
    return item


items.sort(key=sort_key)
assert seen == [0] * 9
assert items == [0, 4, 6, 10, 12, 13, 20, 21, 22]

items[0] = 2**100
items[1:3] = [False, 3.5]
assert type(items[1]) is bool
assert type(items[2]) is float

# A packed list can widen to hold references to itself.
cyclic = [1, 2]
cyclic.append(cyclic)
assert cyclic[-1] is cyclic
assert repr(cyclic) == '[1, 2, [...]]'
cyclic.clear()

# Mixing integers and booleans preserves each item's type.
assert repr([1, None] + [True]) == '[1, None, True]'
flags = [None, True]
flags += [0]
assert repr(flags) == '[None, True, 0]'
assert repr(sorted([True, 1, False, 0])) == '[False, 0, True, 1]'

bounds = [-(2**63), None, 2**63 - 1]
assert bounds.copy() == bounds and bounds[:0] == [] and [] + bounds == bounds

nones = [None] * 128
assert (nones + nones) == [None] * 256
ints = nones.copy()
ints[64] = 0
flags = nones.copy()
flags.insert(63, True)
assert ints[63:66] == [None, 0, None] and type(ints[64]) is int
assert flags[62:65] == [None, True, None] and flags[63] is True

# Exercise bitmap moves across word boundaries.
for pattern, insertions in [((None, 0, 1, -1, 2), (3, 2, None, 1)), ((None, False, True), (False, True, None))]:
    original = pattern * 56
    replacements = insertions * 24
    sample = list(original)
    for index in [0, 1, 62, 63, 64, 65, 127, 128, len(sample)]:
        sample.insert(index, None)
        assert sample.pop(index) is None
        assert repr(tuple(sample)) == repr(original)
    sample.reverse()
    assert repr(tuple(sample)) == repr(original[::-1])
    sample[65:61:-1] = replacements[:4]
    expected = list(original[::-1])
    for i, j in enumerate(range(65, 61, -1)):
        expected[j] = replacements[i]
    assert repr(sample) == repr(expected)

    for start in [0, 61, 63, 64, 65, 125, 127, 128]:
        for removed in [0, 1, 17, 33]:
            for added in [0, 1, 7, 63, 64, 65]:
                end = start + removed
                sample = list(original)
                sample[start:end] = replacements[:added]
                expected = original[:start] + replacements[:added] + original[end:]
                assert repr(tuple(sample.copy())) == repr(expected)

    sample = list(pattern)
    sample += sample
    sample *= 2
    assert repr(tuple(sample)) == repr(pattern * 4)
    repeated = list(pattern) * 1000
    assert len(repeated) == len(pattern) * 1000
    for offset, value in enumerate(pattern):
        assert all(type(item) is type(value) and item == value for item in repeated[offset :: len(pattern)])

# Iterators and None values survive widening from byte slots.
raw = b'\x00\x01\xfe\xff'
small = list(raw)
assert small == [0, 1, 254, 255] and all(type(x) is int for x in small)
assert bytes(small[::-1]) == b'\xff\xfe\x01\x00'
assert list(b'') == []
iterator = iter(small)
assert next(iterator) == 0
small[1] = 256
small.append(-1)
assert list(iterator) == [256, 254, 255, -1]
assert raw == b'\x00\x01\xfe\xff'

small = [255, None, 0] * 70
small[63:66] = [256, None, -1, 255]
assert small[62:68] == [0, 256, None, -1, 255, 255]
assert small[::3].copy()[21] == 256
assert [None, 255] + [256, None] == [None, 255, 256, None]

small = [1] * 70
small += [256, None]
assert small[69:] == [1, 256, None]
small = [255, None]
small.extend([-1, 0])
assert small == [255, None, -1, 0]
small.append(True)
assert type(small[-1]) is bool

# Exercise self-assignment on packed slices.
small = [0, 1, 2, 3, 4, 5]
small[::-1] = small
assert small == [5, 4, 3, 2, 1, 0]
small[1:2] = [30, 31, 32]
assert small == [5, 30, 31, 32, 3, 2, 1, 0]
small[1:4] = [4]
assert small == [5, 4, 3, 2, 1, 0]

wide = [256, 300, 400, 512]
wide[1::2] = [3, 4]
assert wide == [256, 3, 400, 4]
wide[::-1] = wide
assert wide == [4, 400, 3, 256]
empty = []
empty[:] = wide
assert empty == wide and empty is not wide

for pattern in [(None, 7, 0), (None, -257, 258), (None, True, False), (True, False, True)]:
    expected = pattern * 90
    source = list(expected)
    assert tuple(source[17:214]) == expected[17:214]

assert list(range(0)) == []
assert list(range(3, 3, -1)) == []
assert list(range(255, -1, -17)) == [255 - i * 17 for i in range(16)]
assert list(range(1, 258, 127)) == [1, 128, 255]
assert list(range(-(2**63), -(2**63) + 4, 2)) == [-(2**63), -(2**63) + 2]
assert list(range(2**63 - 1, 2**63 - 5, -2)) == [2**63 - 1, 2**63 - 3]

flags = [None, False, True]
flags_copy = list(flags)
flags_copy[0] = True
assert flags == [None, False, True] and flags_copy == [True, False, True]
flags_copy *= 701
assert all(item is True for item in flags_copy[::3])
assert all(item is False for item in flags_copy[1::3])
assert all(item is True for item in flags_copy[2::3])

nones = [None] * 129
alias = nones
nones *= 23
assert alias is nones and len(alias) == 129 * 23 and alias[-1] is None

shared = {}
mixed = [shared, 1] * 100
for copied in (list(mixed), mixed[2:150], mixed[-2::-2]):
    assert copied is not mixed and copied[0] is shared
    copied.pop()
assert len(mixed) == 200 and mixed[170:10] == []

stack = []
for value in (255, -1, 0, 256, True, shared):
    stack.append(value)
    result = stack.pop()
    assert result == value and type(result) is type(value) and stack == []

for pattern in ((-1, None), (0, None), (True, None), (shared, 0)):
    items = list(pattern * 40)
    alias = items
    items[:] = []
    items.extend(pattern)
    assert items == list(pattern)
    items *= 0
    items += [False, True]
    assert alias == [False, True] and alias[0] is False

drained = list(range(80))
drained.clear()
fresh = []
fresh[:] = drained
assert fresh == []
drained[:] = drained
drained.extend([True, None])
assert drained == [True, None] and drained[0] is True
