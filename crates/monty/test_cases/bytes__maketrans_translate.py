# === bytes.maketrans() ===
ident = bytes.maketrans(b'', b'')
assert len(ident) == 256
assert ident == bytes(range(256))

tab = bytes.maketrans(b'abc', b'xyz')
assert len(tab) == 256
assert tab[97:100] == b'xyz'
assert tab[0] == 0 and tab[255] == 255

# Later duplicate in `frm` wins
dup = bytes.maketrans(b'aa', b'xy')
assert dup[97] == ord('y')

# Callable on bytes instance as well as type
assert b'hello'.maketrans(b'h', b'H') == bytes.maketrans(b'h', b'H')

# === bytes.translate() ===
assert b'hello world'.translate(bytes.maketrans(b'hw', b'HW')) == b'Hello World'
assert b'hello world'.translate(None) == b'hello world'
assert b'hello world'.translate(None, b'lo') == b'he wrd'
assert b'hello world'.translate(None, delete=b'lo') == b'he wrd'
assert b'hello world'.translate(None, delete=b'') == b'hello world'

# Deletion happens BEFORE translation
assert b'hello'.translate(bytes.maketrans(b'h', b'l'), b'l') == b'leo'
assert b'hello'.translate(bytes.maketrans(b'h', b'l'), delete=b'hl') == b'eo'

# Non-ASCII bytes
high_tab = bytes.maketrans(b'\x00\xff', b'\xff\x00')
assert b'\x00\x01\xff'.translate(high_tab, b'\x01') == b'\xff\x00'

# === Error cases ===
try:
    bytes.maketrans()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'maketrans expected 2 arguments, got 0'

try:
    bytes.maketrans(b'a')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'maketrans expected 2 arguments, got 1'

try:
    bytes.maketrans(b'a', b'b', b'c')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'maketrans expected 2 arguments, got 3'

try:
    bytes.maketrans(frm=b'a', to=b'b')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'bytes.maketrans() takes no keyword arguments'

try:
    b''.maketrans(frm=b'a', to=b'b')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'bytes.maketrans() takes no keyword arguments'

try:
    bytes.maketrans('a', b'b')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "a bytes-like object is required, not 'str'"

try:
    bytes.maketrans(b'a', 123)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "a bytes-like object is required, not 'int'"

try:
    bytes.maketrans(b'a', b'bc')
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'maketrans arguments must have same length'

try:
    b'abc'.translate()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'translate() takes at least 1 positional argument (0 given)'

try:
    b'abc'.translate(table=None)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'translate() takes at least 1 positional argument (0 given)'

try:
    b'abc'.translate(None, b'a', b'b')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'translate() takes at most 2 arguments (3 given)'

try:
    b'abc'.translate(None, foo=b'a')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "translate() got an unexpected keyword argument 'foo'"

try:
    b'abc'.translate('x' * 256)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "a bytes-like object is required, not 'str'"

try:
    b'abc'.translate(b'short')
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'translation table must be 256 characters long'

try:
    b'abc'.translate(b'short', 123)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'translation table must be 256 characters long'

try:
    b'abc'.translate(None, 'a')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "a bytes-like object is required, not 'str'"

try:
    b'abc'.translate(None, None)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "a bytes-like object is required, not 'NoneType'"

try:
    b'abc'.translate(None, 123)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "a bytes-like object is required, not 'int'"
