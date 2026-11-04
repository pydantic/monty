import fnmatch
import os

value = bytearray(b'a.txt')
for code in [
    'os.fspath(value)',
    'os.path.basename(value)',
    'os.path.commonprefix([value])',
    'fnmatch.fnmatch(value, b"*")',
]:
    try:
        eval(code)
    except TypeError as exc:
        assert str(exc) == 'expected str, bytes or os.PathLike object, not bytearray'
    else:
        assert False, code

assert fnmatch.fnmatchcase(value, b'*.txt')
assert fnmatch.filter([value], b'*.txt')[0] is value
assert fnmatch.fnmatchcase(memoryview(value), b'*.txt')
try:
    fnmatch.fnmatchcase(b'a.txt', bytearray(b'*.txt'))
except TypeError as exc:
    assert str(exc) == "unhashable type: 'bytearray'"
else:
    assert False

try:
    fnmatch.translate(bytearray(b'*'))
except TypeError as exc:
    assert str(exc) == 'decoding to str: need a bytes-like object, int found'
else:
    assert False
assert fnmatch.translate(bytearray()) == fnmatch.translate('')
