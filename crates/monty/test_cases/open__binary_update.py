# /// monty
# mount-fs = true
# ///
from pathlib import Path

path = root / 'update.bin'
path.write_bytes(b'old contents')
with path.open('w+b') as f:
    assert f.read() == b''
    assert f.write(b'abcdef') == 6
    f.seek(0)
    old = f.read()
    f.seek(2)
    assert f.write(b'XY') == 2
    assert f.tell() == 4
    assert f.read() == b'ef'
    f.seek(8)
    f.write(b'Z')
    f.seek(100)
    assert f.write(b'') == 0
    assert f.seek(0, 2) == 9
    f.seek(0)
    assert f.read() == b'abXYef\0\0Z'
    assert old == b'abcdef'

with open(path, 'wb') as f:
    f.write(b'abcdef')
    f.seek(2)
    f.write(b'XY')
assert path.read_bytes() == b'abXYef'

with Path(path).open('wb+') as f:
    assert f.read() == b''
    f.write(b'one\ntwo\n')
    f.seek(0)
    assert list(f) == [b'one\n', b'two\n']
    f.seek(0)
    f.write(b'ONE')
    f.seek(0)
    assert list(f) == [b'ONE\n', b'two\n']
