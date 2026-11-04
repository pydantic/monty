formats = ['B', 'b', 'c', '@B', '@b', '@c']
raw = bytes(bytearray(range(256)))
for fmt in formats:
    view = memoryview(raw).cast(fmt)
    assert view.format == fmt
    if fmt[-1] == 'B':
        assert view.tolist() == list(range(256))
    elif fmt[-1] == 'b':
        assert view.tolist() == [i if i < 128 else i - 256 for i in range(256)]
    else:
        assert view.tolist() == [raw[i : i + 1] for i in range(256)]
    assert view.tobytes() == raw
    assert hash(view) == hash(raw)
    for other_fmt in formats:
        owner = bytearray(256)
        left = memoryview(owner).cast(fmt)
        right = memoryview(raw).cast(other_fmt)
        if fmt[-1] == other_fmt[-1]:
            left[:] = right
            assert owner == raw
        else:
            try:
                left[:] = right
            except ValueError as exc:
                assert str(exc) == 'memoryview assignment: lvalue and rvalue have different structures'
            else:
                assert False, (fmt, other_fmt)
        assert (view == right) == (view.tolist() == right.tolist())
        left.release()
        right.release()
        owner.append(0)
    view.release()

for fmt in formats:
    view = memoryview(bytearray(b'x')).cast(fmt)
    for value in [1.5, 2**100, -129, 256, bytearray(b'A'), memoryview(b'A')]:
        kind = 'value' if fmt[-1] != 'c' and isinstance(value, int) else 'type'
        try:
            view[0] = value
        except (TypeError, ValueError) as exc:
            assert isinstance(exc, ValueError if kind == 'value' else TypeError)
            assert str(exc) == f"memoryview: invalid {kind} for format '{fmt[-1]}'"
        else:
            assert False, (fmt, value)
    view.release()
