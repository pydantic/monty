for code, itemsize in [('B', 1), ('H', 2), ('I', 4)]:
    count = 16 // itemsize
    for shape in [[count], [2, count // 2]]:
        base = memoryview(bytearray(range(16))).cast(code, shape)
        for step in [1, 2, -1, -2, 2**63 - 1, -(2**63)]:
            empty = base[0:0:step]
            expected_stride = base.strides[0] * max(step, -(2**63 - 1))
            expected_stride = (expected_stride + 2**63) % (2**64) - 2**63
            assert empty.strides[0] == expected_stride
            contiguous = len(shape) > 1 or expected_stride == itemsize
            assert empty.c_contiguous is contiguous, (code, shape, step, empty.strides, empty.c_contiguous)
            assert empty.f_contiguous is contiguous
            assert empty.contiguous is contiguous
            assert empty.nbytes == 0
            assert empty.tobytes('C') == empty.tobytes('F') == b''
            assert empty.tolist() == []
            if len(shape) == 1 and contiguous:
                assert empty.cast('B').shape == (0,)
            else:
                try:
                    empty.cast('B')
                except TypeError as exc:
                    expected = (
                        'memoryview: cannot cast view with zeros in shape or strides'
                        if len(shape) > 1
                        else 'memoryview: casts are restricted to C-contiguous views'
                    )
                    assert str(exc) == expected
                else:
                    assert False
