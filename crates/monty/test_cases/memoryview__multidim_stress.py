def product(shape):
    result = 1
    for dimension in shape:
        result *= dimension
    return result


def nested(values, shape):
    if len(shape) == 1:
        return list(values)
    inner = product(shape[1:])
    return [nested(values[i * inner : (i + 1) * inner], shape[1:]) for i in range(shape[0])]


def fortran_bytes(units, shape, rows):
    logical_shape = [len(rows)] + shape[1:]
    result = []
    for flat in range(product(logical_shape)):
        coordinates = []
        for dimension in logical_shape:
            coordinates.append(flat % dimension)
            flat //= dimension
        coordinates[0] = rows[coordinates[0]]
        source = 0
        for dimension, coordinate in zip(shape, coordinates):
            source = source * dimension + coordinate
        result.append(units[source])
    return b''.join(result)


for code, size in [('B', 1), ('H', 2), ('I', 4), ('Q', 8), ('e', 2), ('f', 4), ('d', 8)]:
    for shape in [[2, 3], [3, 1, 4], [2, 3, 4], [1, 2, 1, 3]]:
        count = product(shape)
        owner = bytearray(count * size)
        flat = memoryview(owner).cast(code)
        for i in range(count):
            flat[i] = i
        original = bytes(owner)
        units = [original[i * size : (i + 1) * size] for i in range(count)]
        matrix = memoryview(owner).cast(code, shape)
        assert matrix.shape == tuple(shape) and matrix.itemsize == size
        assert matrix.tolist() == nested(list(range(count)), shape)
        assert matrix.tobytes(order='F') == fortran_bytes(units, shape, list(range(shape[0])))
        assert matrix.cast('B').cast(code).tolist() == list(range(count))
        for position in range(count):
            remaining = position
            coordinates = []
            for dimension in shape[::-1]:
                coordinates.append(remaining % dimension)
                remaining //= dimension
            assert matrix[tuple(coordinates[::-1])] == position
        for start in [None, -5, 0, 1, 5]:
            for stop in [None, -1, 0, 2, 5]:
                for step in [-(2**63), -3, -1, 1, 2, 2**63 - 1]:
                    key = slice(start, stop, step)
                    rows = list(range(shape[0]))[key]
                    sliced = matrix[key]
                    expected = nested(list(range(count)), shape)[key]
                    assert sliced.tolist() == expected, (code, shape, start, stop, step)
                    assert sliced.shape == tuple([len(rows)] + shape[1:])
                    assert sliced.tobytes(order='F') == fortran_bytes(units, shape, rows)
                    sliced.release()
        expected = list(range(count))
        expected[::2] = expected[::-2]
        flat[::2] = flat[::-2]
        assert flat.tolist() == expected
        flat.release()
        matrix.release()
        owner.append(0)
