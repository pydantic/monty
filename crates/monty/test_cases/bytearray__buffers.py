# Join cases adapted from CPython 3.14 Lib/test/test_bytes.py, BaseBytesTest.test_join.
# https://github.com/python/cpython/blob/3.14/Lib/test/test_bytes.py
for constructor in [bytes, bytearray]:
    for parts in [[], [b''], [b'abc'], [b'a', b'bc'], [b'ab', b'c'], [b'a', b'b', b'c']]:
        buffers = [bytearray(part) for part in parts]
        expected = b''.join(parts)
        for items in [buffers, tuple(buffers), iter(buffers)]:
            result = constructor(b'').join(items)
            assert result == expected
            assert type(result) is constructor

    separator = constructor(b'.:')
    assert separator.join([bytearray(b'ab'), b'cd']) == b'ab.:cd'
    assert separator.join([b'ab', bytearray(b'cd')]) == b'ab.:cd'
    try:
        separator.join([bytearray(b'ab'), 'cd', b'ef'])
    except TypeError:
        pass
    else:
        assert False

for value in [b'123', b'  -123 ', b'1_234']:
    assert int(bytearray(value)) == int(value)
for base, value in [(0, b'0xff'), (2, b'101'), (16, b'face')]:
    assert int(bytearray(value), base) == int(value, base)
for value in [b'1.5', b' -1.2e3 ', b'1_234.5', b'inf']:
    assert float(bytearray(value)) == float(value)
assert str(bytearray('café', 'utf-8'), 'utf-8') == 'café'
assert str(bytearray(b'\xffok'), 'ascii', 'ignore') == 'ok'


class Base:
    def __index__(self):
        number[:] = b'ff'
        return 16


number = bytearray(b'10')
assert int(number, Base()) == 255


class Items:
    def __init__(self):
        self.index = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.index += 1
        if self.index == 1:
            return item
        if self.index == 2:
            item[:] = b'changed'
            separator[:] = b':'
            return b'end'
        raise StopIteration


item = bytearray(b'start')
separator = bytearray(b',')
assert separator.join(Items()) == b'changed:end'
