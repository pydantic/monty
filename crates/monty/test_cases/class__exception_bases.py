# Test class creation with exception bases and MRO layout conflict check.

# 1. Valid single exception base
class MyBaseErr(BaseException):
    pass

class MyErr(Exception):
    pass

class MyValueErr(ValueError):
    pass

class MyKeyErr(KeyError):
    pass

class MyOsErr(OSError):
    pass


# 2. Valid multi-base without conflict
# Plain mixin + exception
class Mixin:
    pass

class MyMixinErr(Mixin, ValueError):
    pass

assert issubclass(MyMixinErr, Mixin)
assert issubclass(MyMixinErr, ValueError)

# Types sharing BaseException layout combine freely
class KeyAndValue(KeyError, ValueError):
    pass

assert issubclass(KeyAndValue, KeyError)
assert issubclass(KeyAndValue, ValueError)

class OsAndValue(OSError, ValueError):
    pass

assert issubclass(OsAndValue, OSError)
assert issubclass(OsAndValue, ValueError)

# Same layout group combines
class SameGroup(FileNotFoundError, OSError):
    pass

assert issubclass(SameGroup, FileNotFoundError)
assert issubclass(SameGroup, OSError)


# 3. Invalid layout conflict pairs
# Direct conflicts
invalid_pairs = [
    (OSError, SyntaxError),
    (OSError, NameError),
    (AttributeError, StopIteration),
    (ImportError, SystemExit),
    (UnicodeDecodeError, UnicodeEncodeError),
    (StopIteration, SystemExit),
]

for base1, base2 in invalid_pairs:
    try:
        type('Conflict', (base1, base2), {})
        assert False, f'expected layout conflict for {base1} and {base2}'
    except TypeError as e:
        assert str(e) == 'multiple bases have instance lay-out conflict'

# Indirect layout conflict: A(OSError) and B(SyntaxError)
class A(OSError):
    pass

class B(SyntaxError):
    pass

try:
    class IndirectConflict(A, B):
        pass
    assert False, 'expected indirect layout conflict'
except TypeError as e:
    assert str(e) == 'multiple bases have instance lay-out conflict'


# 4. Guard: object.__new__(MyErr)
try:
    object.__new__(MyErr)
    assert False, 'expected TypeError for object.__new__(MyErr)'
except TypeError as e:
    assert str(e) == 'object.__new__(MyErr) is not safe, use MyErr.__new__()'

class MyErrWithNew(Exception):
    def __new__(cls):
        pass

try:
    object.__new__(MyErrWithNew)
    assert False, 'expected TypeError for object.__new__(MyErrWithNew)'
except TypeError as e:
    assert str(e) == 'object.__new__(MyErrWithNew) is not safe, use Exception.__new__()'


# 5. Guard: inst.__class__ = MyErr
class Plain:
    pass

p = Plain()
try:
    p.__class__ = MyErr
    assert False, 'expected TypeError for __class__ assignment across layouts'
except TypeError as e:
    assert str(e) == "__class__ assignment: 'MyErr' object layout differs from 'Plain'"
