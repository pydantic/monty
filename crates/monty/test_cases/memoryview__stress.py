bounds = [None, -20, -1, 0, 1, 4, 20]
steps = [-(2**63), -7, -2, -1, 1, 2, 7, 2**63 - 1]
for size in range(8):
    owner = bytearray(range(size))
    original = bytes(owner)
    root = memoryview(owner)
    for start in bounds:
        for stop in bounds:
            for step in steps:
                key = slice(start, stop, step)
                cut = root[key]
                expected = original[key]
                assert cut.tobytes() == expected, (size, start, stop, step)
                assert len(cut) == len(expected)
                assert cut.nbytes == len(expected)
                assert cut.shape == (len(expected),)
                assert cut.tolist() == list(expected)
                assert cut == expected
                for nested_key in [slice(None, None, -1), slice(None, None, 2), slice(1, 1)]:
                    nested = cut[nested_key]
                    assert nested.tobytes() == expected[nested_key]
                    nested.release()
                cut.release()
    root.release()
    owner.append(255)
    assert owner[-1] == 255

for step in [-3, -2, -1, 1, 2, 3]:
    for start in [None, -3, 0, 2, 20]:
        for stop in [None, -3, 0, 2, 20]:
            owner = bytearray(b'abcdefgh')
            view = memoryview(owner)
            key = slice(start, stop, step)
            positions = list(range(8))[key]
            rhs = bytearray([70 + i for i in range(len(positions))])
            expected = list(b'abcdefgh')
            for position, byte in zip(positions, rhs):
                expected[position] = byte
            view[key] = rhs
            assert list(owner) == expected, (start, stop, step)
            assert view[key].tobytes() == bytes(rhs)
            view.release()
            owner.append(0)

for operation in ['left', 'right', 'reverse', 'self']:
    owner = bytearray(b'abcdef')
    view = memoryview(owner)
    if operation == 'left':
        view[:-1] = view[1:]
        assert owner == b'bcdeff'
    elif operation == 'right':
        view[1:] = view[:-1]
        assert owner == b'aabcde'
    elif operation == 'reverse':
        view[::-1] = view
        assert owner == b'fedcba'
    else:
        view[:] = view
        assert owner == b'abcdef'
    before = bytes(owner)
    try:
        view[:] = b'x'
    except ValueError:
        pass
    else:
        assert False
    assert owner == before
    view.release()
    owner.append(0)

for order in [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]]:
    for release in [True, False]:
        owner = bytearray(b'abcd')
        a = memoryview(owner)
        b = memoryview(a)[::-1]
        c = b.toreadonly()
        views = [a, b, c]
        a = None
        b = None
        c = None
        for index in order[:-1]:
            if release:
                views[index].release()
            views[index] = None
            try:
                owner.append(0)
            except BufferError:
                pass
            else:
                assert False, order
        if release:
            views[order[-1]].release()
        views[order[-1]] = None
        owner.append(0)
        assert owner == b'abcd\0'
