# /// monty
# skip-cpython-windows = true
# ///


class BadIndex:
    def __index__(self):
        return []


class BadFloat:
    def __float__(self):
        return 1


class FalseBool:
    def __bool__(self):
        return False


class BadBool:
    def __bool__(self):
        return []


class ZeroLength:
    def __len__(self):
        return 0


class NegativeLength:
    def __len__(self):
        return -1


class HugeLength:
    def __len__(self):
        return 2**100


class UserError:
    def __index__(self):
        raise error_kind('custom index')

    def __float__(self):
        raise error_kind('custom float')

    def __bool__(self):
        raise error_kind('custom bool')


for code in 'BbhHiIlLqQnNefdP':
    view = memoryview(bytearray(8)).cast(code)
    for value in [None, 'x', b'x', bytearray(b'x'), memoryview(b'x'), [], BadIndex(), BadFloat()]:
        try:
            view[0] = value
        except TypeError as exc:
            assert str(exc) == "memoryview: invalid type for format '" + code + "'", (code, str(exc))
        else:
            assert False, code

for value in [None, 'x', b'', b'xx', bytearray(b'x'), memoryview(b'x'), 120]:
    view = memoryview(bytearray(1)).cast('c')
    try:
        view[0] = value
    except (TypeError, ValueError) as exc:
        if isinstance(value, bytes):
            assert isinstance(exc, ValueError)
            assert str(exc) == "memoryview: invalid value for format 'c'"
        else:
            assert isinstance(exc, TypeError)
            assert str(exc) == "memoryview: invalid type for format 'c'"
    else:
        assert False

for code in ['B', 'h', 'i', 'l', 'q', 'n', 'L', 'Q', 'N', 'e', 'f', 'd', '?', 'P']:
    view = memoryview(bytearray(8)).cast(code)
    for error_kind in [TypeError, ValueError, OverflowError, RuntimeError]:
        try:
            view[0] = UserError()
        except Exception as exc:
            if code == 'P':
                assert isinstance(exc, TypeError)
                assert str(exc) == "memoryview: invalid type for format 'P'"
            elif code == '?':
                assert isinstance(exc, error_kind)
                assert str(exc) == 'custom bool'
            elif error_kind is RuntimeError:
                assert isinstance(exc, RuntimeError)
                assert str(exc) == ('custom float' if code in 'efd' else 'custom index')
            else:
                expected_kind = 'type' if error_kind is TypeError else 'value'
                assert isinstance(exc, TypeError if error_kind is TypeError else ValueError)
                assert str(exc) == 'memoryview: invalid ' + expected_kind + " for format '" + code + "'"
        else:
            assert False, code


class ReleaseIndex:
    def __index__(self):
        view.release()
        owner.clear()
        return 1


class ReleaseFloat:
    def __float__(self):
        view.release()
        owner.clear()
        return 1.0


class ReleaseBool:
    def __bool__(self):
        view.release()
        owner.clear()
        return True


for code, value in [('i', ReleaseIndex()), ('d', ReleaseFloat()), ('?', ReleaseBool())]:
    owner = bytearray(8)
    view = memoryview(owner).cast(code)
    try:
        view[0] = value
    except ValueError as exc:
        assert str(exc) == 'operation forbidden on released memoryview object'
    else:
        assert False, code
    assert owner == b''

view = memoryview(bytearray(1)).cast('?')
for value in [FalseBool(), ZeroLength()]:
    view[0] = value
    assert view[0] is False
for value, kind, message in [
    (BadBool(), TypeError, '__bool__ should return bool, returned list'),
    (NegativeLength(), ValueError, '__len__() should return >= 0'),
    (HugeLength(), OverflowError, "cannot fit 'int' into an index-sized integer"),
]:
    try:
        view[0] = value
    except Exception as exc:
        assert isinstance(exc, kind)
        assert str(exc) == message
    else:
        assert False
