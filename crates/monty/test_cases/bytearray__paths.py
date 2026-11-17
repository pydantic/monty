import fnmatch
import os.path

for operation in ['basename', 'join', 'fnmatch']:
    try:
        if operation == 'basename':
            os.path.basename(bytearray(b'/a/b'))
        elif operation == 'join':
            os.path.join(bytearray(b'/a'), b'b')
        else:
            fnmatch.fnmatch(bytearray(b'abc'), b'a*')
    except TypeError:
        pass
    else:
        assert False, operation

assert os.path.basename(b'/a/b') == b'b'
assert os.path.join(b'/a', b'b') == os.path.join('/a', 'b').encode()
assert fnmatch.fnmatch(b'abc', b'a*')
