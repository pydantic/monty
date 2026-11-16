# Adapted from CPython 3.14 Lib/test/test_bytes.py, BaseBytesTest search methods.
# https://github.com/python/cpython/blob/3.14/Lib/test/test_bytes.py


def search(method, data, needle, start=None, end=None):
    if method == 'find':
        return data.find(needle, start, end)
    if method == 'rfind':
        return data.rfind(needle, start, end)
    if method == 'count':
        return data.count(needle, start, end)
    if method == 'index':
        return data.index(needle, start, end)
    return data.rindex(needle, start, end)


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


class IntOnly:
    def __int__(self):
        return 105


class RaisingIndex:
    def __index__(self):
        raise ZeroDivisionError('index failed')


for factory in [bytes, bytearray]:
    data = factory(b'mississippi')
    for method, expected, bounded in [
        ('find', 1, 7),
        ('rfind', 10, 7),
        ('count', 4, 1),
        ('index', 1, 7),
        ('rindex', 10, 7),
    ]:
        for needle in [105, Index(105), b'i', bytearray(b'i')]:
            assert search(method, data, needle) == expected
            assert search(method, data, needle, -5, -1) == bounded
        for absent in [119, Index(119)]:
            try:
                result = search(method, data, absent)
            except ValueError:
                assert method in ['index', 'rindex']
            else:
                assert method not in ['index', 'rindex']
                assert result == (0 if method == 'count' else -1)
        for value in [-1, 256, 2**100, -(2**100)]:
            for needle in [value, Index(value)]:
                try:
                    search(method, data, needle, 100, 0)
                except ValueError as exc:
                    assert str(exc) == 'byte must be in range(0, 256)'
                else:
                    assert False, (method, needle)
        for needle in [1.5, 'i', None, IntOnly(), Index('i')]:
            try:
                search(method, data, needle)
            except TypeError:
                pass
            else:
                assert False, (method, needle)
        try:
            search(method, data, RaisingIndex())
        except ZeroDivisionError as exc:
            assert str(exc) == 'index failed'
        else:
            assert False

    data = factory(b'\x00\x01\xff\x01')
    assert data.find(False) == 0
    assert data.rfind(True) == 3
    assert data.count(True) == 2
    assert data.index(255) == 2
    assert data.rindex(Index(255)) == 2

# Bounds convert before the needle; same-size mutations during conversion remain visible.
calls = []
data = bytearray(b'abc')


class Bound:
    def __init__(self, name, value):
        self.name = name
        self.value = value

    def __index__(self):
        calls.append(self.name)
        if self.name == 'start':
            data.extend(b'abc')
        return self.value


class Needle:
    def __index__(self):
        calls.append('needle')
        data[3] = 120
        return 120


assert data.find(Needle(), Bound('start', 3), Bound('end', 6)) == 3
assert calls == ['start', 'end', 'needle']
data.append(33)
assert data == b'abcxbc!'
