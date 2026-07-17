# call-external
# `__enter__` and `__exit__` run as real pushed frames (unlike
# `__repr__`/`__str__`), so they can suspend on external function calls and
# resume mid-`with`. `add_ints`/`concat_strings` are external functions
# resolved by the host on the Monty side and plain functions on the CPython
# side.


class Resource:
    def __init__(self, base):
        self.base = base
        self.closed = False

    def __enter__(self):
        # Suspends inside the BeforeWith opcode's pushed frame.
        self.value = add_ints(self.base, 100)
        return self.value

    def __exit__(self, typ, val, tb):
        # Suspends inside the WithExit / WithExceptStart pushed frame.
        self.closed = concat_strings('closed:', str(typ))
        return None


r = Resource(7)
with r as v:
    assert v == 107
    assert r.value == 107
assert r.closed == 'closed:None'

# Exception path: __exit__ still suspends and sees the exception type.
r2 = Resource(1)
caught = None
try:
    with r2:
        raise ValueError('boom')
except ValueError as e:
    caught = str(e)
assert caught == 'boom'
assert r2.closed == "closed:<class 'ValueError'>"


# A suspending __exit__ that swallows the exception.
class Swallow:
    def __enter__(self):
        return self

    def __exit__(self, typ, val, tb):
        self.note = add_ints(20, 22)
        return typ is not None


s = Swallow()
with s:
    raise ValueError('to-swallow')
assert s.note == 42


# Host name lookup (not a call) inside the dunders: resolving an unknown
# global suspends to the host's NameLookup handler mid-`with`, a different
# suspension path from an external function call. `CONST_INT`/`CONST_STR`
# are host-injected constants.
class NameLookupCm:
    def __enter__(self):
        return CONST_INT + 1

    def __exit__(self, typ, val, tb):
        self.closed_with = CONST_STR
        return None


n = NameLookupCm()
with n as v:
    assert v == 43
assert n.closed_with == 'hello'
