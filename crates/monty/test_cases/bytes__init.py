# === bytes() constructor tests ===

# --- Empty and default construction ---
assert bytes() == b''
assert bytes(0) == b''
assert bytes(False) == b''
assert bytes(b'') == b''
assert bytes(b'abc') == b'abc'

# --- bytes(int) length construction ---
assert bytes(1) == b'\x00'
assert bytes(True) == b'\x00'
assert bytes(3) == b'\x00\x00\x00'
assert bytes(5) == b'\x00\x00\x00\x00\x00'


# Custom __index__ for size
class SizeObj:
    def __init__(self, n):
        self.n = n

    def __index__(self):
        return self.n


assert bytes(SizeObj(0)) == b''
assert bytes(SizeObj(4)) == b'\x00\x00\x00\x00'

# --- bytes(iterable) of ints ---
assert bytes([]) == b''
assert bytes(()) == b''
assert bytes([65]) == b'A'
assert bytes([65, 66, 67]) == b'ABC'
assert bytes((65, 66, 67)) == b'ABC'
assert bytes([0, 255]) == b'\x00\xff'
assert bytes([True, False, True]) == b'\x01\x00\x01'
assert bytes(range(0)) == b''
assert bytes(range(4)) == b'\x00\x01\x02\x03'
assert bytes(range(65, 69)) == b'ABCD'

# Iterators and generators
assert bytes(iter([])) == b''
assert bytes(iter([65, 66])) == b'AB'
assert bytes(x for x in [72, 101, 108, 108, 111]) == b'Hello'
assert bytes(x for x in []) == b''

# Collections: set, dict, frozenset
assert bytes({65}) == b'A'
assert bytes(frozenset([66])) == b'B'
assert bytes({65: 1, 66: 2}) == b'AB'
assert bytes({65: 1, 66: 2}.keys()) == b'AB'
assert bytes({1: 65, 2: 66}.values()) == b'AB'


# Custom iterable
class CustomIter:
    def __init__(self):
        self.items = [88, 89, 90]
        self.idx = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self.idx >= len(self.items):
            raise StopIteration
        val = self.items[self.idx]
        self.idx += 1
        return val


assert bytes(CustomIter()) == b'XYZ'


# Custom element with __index__
class ByteElem:
    def __init__(self, val):
        self.val = val

    def __index__(self):
        return self.val


assert bytes([ByteElem(65), ByteElem(66)]) == b'AB'


# Fallback: __index__ raising TypeError falls back to __iter__
class FallbackIter:
    def __index__(self):
        raise TypeError('not indexable')

    def __iter__(self):
        return iter([65, 66])


assert bytes(FallbackIter()) == b'AB'


# Precedence: __index__ takes precedence over __iter__
class IndexOverIter:
    def __index__(self):
        return 2

    def __iter__(self):
        return iter([65, 66])


assert bytes(IndexOverIter()) == b'\x00\x00'

# --- Error handling ---


def check_exc(fn, exc_type, msg):
    try:
        fn()
        assert False, f'expected {exc_type.__name__} was not raised'
    except exc_type as e:
        assert str(e) == msg, f'unexpected message: got {str(e)!r}, expected {msg!r}'


# Range errors on elements
check_exc(lambda: bytes([256]), ValueError, 'bytes must be in range(0, 256)')
check_exc(lambda: bytes([-1]), ValueError, 'bytes must be in range(0, 256)')
check_exc(lambda: bytes([0, 256]), ValueError, 'bytes must be in range(0, 256)')
check_exc(lambda: bytes([10**30]), ValueError, 'bytes must be in range(0, 256)')
check_exc(lambda: bytes([-(10**30)]), ValueError, 'bytes must be in range(0, 256)')
check_exc(lambda: bytes([ByteElem(300)]), ValueError, 'bytes must be in range(0, 256)')
check_exc(lambda: bytes([ByteElem(-5)]), ValueError, 'bytes must be in range(0, 256)')

# Element type errors
check_exc(lambda: bytes(['a']), TypeError, "'str' object cannot be interpreted as an integer")
check_exc(lambda: bytes([1.5]), TypeError, "'float' object cannot be interpreted as an integer")
check_exc(lambda: bytes([None]), TypeError, "'NoneType' object cannot be interpreted as an integer")
check_exc(lambda: bytes([[]]), TypeError, "'list' object cannot be interpreted as an integer")


class NonIntIndex:
    def __index__(self):
        return 'not an int'


check_exc(lambda: bytes([NonIntIndex()]), TypeError, '__index__ returned non-int (type str)')


# Argument type errors
class PlainObj:
    pass


check_exc(lambda: bytes(1.5), TypeError, "cannot convert 'float' object to bytes")
check_exc(lambda: bytes(None), TypeError, "cannot convert 'NoneType' object to bytes")
check_exc(lambda: bytes(PlainObj()), TypeError, "cannot convert 'PlainObj' object to bytes")


class BadIter:
    def __iter__(self):
        return 42


check_exc(lambda: bytes(BadIter()), TypeError, "cannot convert 'BadIter' object to bytes")

# Integer count errors
check_exc(lambda: bytes(-1), ValueError, 'negative count')
check_exc(lambda: bytes(SizeObj(-1)), ValueError, 'negative count')
check_exc(lambda: bytes(10**30), OverflowError, "cannot fit 'int' into an index-sized integer")
check_exc(lambda: bytes(-(10**30)), OverflowError, "cannot fit 'int' into an index-sized integer")

# Encoding argument errors
check_exc(lambda: bytes([65], encoding='utf-8'), TypeError, 'encoding without a string argument')
check_exc(lambda: bytes([65], errors='strict'), TypeError, 'errors without a string argument')
check_exc(lambda: bytes('hello'), TypeError, 'string argument without an encoding')
