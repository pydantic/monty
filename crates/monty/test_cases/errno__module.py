import errno
from errno import EACCES, EEXIST, ENOENT

assert EACCES == 13
assert EEXIST == 17
assert ENOENT == 2
assert errno.errorcode[EACCES] == 'EACCES'
assert errno.errorcode[EEXIST] == 'EEXIST'
assert errno.errorcode[ENOENT] == 'ENOENT'
assert isinstance(errno.errorcode, dict)

for code, name in errno.errorcode.items():
    assert isinstance(code, int)
    assert isinstance(name, str)
    assert getattr(errno, name) == code

assert 0 not in errno.errorcode
assert errno.errorcode.get(0) is None

assert not hasattr(errno, 'strerror')
