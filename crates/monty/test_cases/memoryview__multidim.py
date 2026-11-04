owner = bytearray(range(24))
root = memoryview(owner)
v = root.cast('B', [2, 3, 4])
assert v.shape == (2, 3, 4) and v.strides == (12, 4, 1)
assert v.ndim == 3 and v.nbytes == 24 and len(v) == 2
assert v.itemsize == 1 and v.c_contiguous and not v.f_contiguous
assert v.tolist() == [
    [[0, 1, 2, 3], [4, 5, 6, 7], [8, 9, 10, 11]],
    [[12, 13, 14, 15], [16, 17, 18, 19], [20, 21, 22, 23]],
]
for i in range(2):
    for j in range(3):
        for k in range(4):
            assert v[i, j, k] == 12 * i + 4 * j + k
assert v[-1, -1, -1] == 23
assert v.tobytes() == bytes(owner)
assert list(v.tobytes(order='F')) == [i + 4 * j + 12 * k for i in range(4) for j in range(3) for k in range(2)]
w = v[::-1]
assert w.shape == (2, 3, 4) and w.strides == (-12, 4, 1)
assert not w.contiguous
assert w.tolist() == v.tolist()[::-1]
assert w.tobytes() == bytes(owner)[12:] + bytes(owner)[:12]
assert w[0, 1, 2] == 18
assert w[:0].tolist() == []
assert w[:0].shape == (0, 3, 4)
assert w[:0].c_contiguous and w[:0].f_contiguous
assert v[1:].tolist() == [v.tolist()[1]]
assert root.cast(format='B', shape=(2, 12)).shape == (2, 12)

words = root.cast('H', [3, 4])
assert words.itemsize == 2 and words.nbytes == 24
assert words.strides == (8, 2)
assert words[1, 2] == 13 * 256 + 12
words[1, 2] = 50000
assert owner[12] == 80 and owner[13] == 195
assert words[1, 2] == 50000
assert words[::-1].tolist() == words.tolist()[::-1]
assert words.tobytes(order='F') == b''.join(
    [words.tobytes()[2 * (4 * i + j) : 2 * (4 * i + j + 1)] for j in range(4) for i in range(3)]
)
readonly = words.toreadonly()
assert readonly.shape == words.shape and readonly.strides == words.strides and readonly.readonly
assert memoryview(words).shape == words.shape
try:
    readonly[0, 0] = 1
except TypeError as exc:
    assert str(exc) == 'cannot modify read-only memory'
else:
    assert False
assert words.cast('B').cast('H', [3, 4]) == words

zero_owner = bytearray(4)
zero = memoryview(zero_owner).cast('I', [])
assert zero.ndim == 0 and zero.shape == () and zero.strides == () and zero.nbytes == 4
assert zero.tolist() == 0
assert zero[()] == 0 and zero[...] is zero
zero[()] = 123
assert zero.tolist() == 123
zero[...] = 456
assert zero[()] == 456
assert zero.cast('B').tobytes() == bytes(zero_owner)
for action in ['len', 'bool', 'iter', 'get']:
    try:
        if action == 'len':
            len(zero)
        elif action == 'bool':
            bool(zero)
        elif action == 'iter':
            iter(zero)
        else:
            zero[0]
    except TypeError:
        pass
    else:
        assert False, action
zero.release()
zero_owner.append(1)


class Wrapped:
    def __iter__(self):
        return iter(memoryview(b'abc'))


assert list(Wrapped()) == [97, 98, 99]

shape = [2, 12]
shaped = root.cast('B', shape)
shape[0] = 999
assert shaped.shape == (2, 12)
shaped.release()
