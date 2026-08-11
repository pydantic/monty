# User-defined classes as context managers: `__enter__` / `__exit__` are
# dispatched by the `with` statement (`BeforeWith` / `WithExit` /
# `WithExceptStart` opcodes) with CPython's type-level lookup and call
# semantics. `__enter__`/`__exit__` run as real frames, so unlike
# `__repr__`/`__str__` they may suspend on external/OS calls (covered by
# `with__class_external.py`).

# === Basic protocol: enter/exit ordering, as-target binding ===
log = []


class Tracked:
    def __init__(self, name):
        self.name = name

    def __enter__(self):
        log.append('enter ' + self.name)
        return self

    def __exit__(self, typ, val, tb):
        log.append('exit ' + self.name)
        return None


with Tracked('a') as bound:
    log.append('body')
    assert bound.name == 'a'
assert log == ['enter a', 'body', 'exit a']


# === __enter__ may return a non-self value ===
class EnterValue:
    def __enter__(self):
        return 42

    def __exit__(self, typ, val, tb):
        return None


with EnterValue() as v:
    assert v == 42


# === __exit__ receives (None, None, None) on the normal path ===
class Grab:
    def __enter__(self):
        return self

    def __exit__(self, typ, val, tb):
        self.typ = typ
        self.val = val
        self.tb = tb
        return True


g = Grab()
with g:
    pass
assert g.typ is None
assert g.val is None
assert g.tb is None

# === __exit__ receives the exception type and value on the exception path ===
# The traceback argument is not asserted: CPython passes a real traceback
# object, Monty always passes None (see limitations/with.md).
g = Grab()
with g:
    raise ValueError('boom')
assert g.typ is ValueError
assert isinstance(g.val, ValueError)
assert str(g.val) == 'boom'

# === Truthy __exit__ return swallows the in-flight exception ===
swallowed = False
with Grab():
    raise ValueError('to-swallow')
    swallowed = False  # unreachable
swallowed = True
assert swallowed


# Any truthy value suppresses, not just True.
class TruthyStr:
    def __enter__(self):
        return self

    def __exit__(self, typ, val, tb):
        return 'truthy'


with TruthyStr():
    raise ValueError('swallowed by str')


# === Falsy/None __exit__ return propagates the exception ===
class Passthrough:
    def __enter__(self):
        return self

    def __exit__(self, typ, val, tb):
        return None


caught = None
try:
    with Passthrough():
        raise ValueError('passthrough-prop')
except ValueError as e:
    caught = str(e)
assert caught == 'passthrough-prop'

# The return value is ignored entirely on the normal-exit path.
with Grab():
    inside = 'ran'
assert inside == 'ran'


# === __enter__ raising skips the body AND __exit__ ===
class BadEnter:
    def __enter__(self):
        raise ValueError('no-entry')

    def __exit__(self, typ, val, tb):
        # Would swallow the ValueError if incorrectly invoked, making the
        # try/except below fail loudly.
        return True


ran_body = False
caught = None
try:
    with BadEnter():
        ran_body = True
except ValueError as e:
    caught = str(e)
assert caught == 'no-entry'
assert not ran_body, 'body skipped when __enter__ raises'


# === __exit__ raising on the normal path propagates ===
class BadExit:
    def __enter__(self):
        return self

    def __exit__(self, typ, val, tb):
        raise ValueError('cleanup-failed')


caught = None
try:
    with BadExit():
        pass
except ValueError as e:
    caught = str(e)
assert caught == 'cleanup-failed'

# === __exit__ raising on the exception path REPLACES the in-flight exception ===
caught_info = None
try:
    with BadExit():
        raise RuntimeError('original')
except ValueError as e:
    caught_info = ('ValueError', str(e))
except RuntimeError as e:
    caught_info = ('RuntimeError', str(e))
assert caught_info == ('ValueError', 'cleanup-failed')


# === Missing dunders: CPython's protocol TypeError, __exit__ checked first ===
class NoEnter:
    def __exit__(self, typ, val, tb):
        return None


class NoExit:
    def __enter__(self):
        return self


class Neither:
    pass


caught = None
try:
    with NoEnter():
        pass
except TypeError as e:
    caught = str(e)
assert caught == "'NoEnter' object does not support the context manager protocol (missed __enter__ method)"

caught = None
try:
    with NoExit():
        pass
except TypeError as e:
    caught = str(e)
assert caught == "'NoExit' object does not support the context manager protocol (missed __exit__ method)"

caught = None
try:
    with Neither():
        pass
except TypeError as e:
    caught = str(e)
assert caught == "'Neither' object does not support the context manager protocol (missed __exit__ method)"


# === Non-callable __enter__ class member ===
class IntEnter:
    __enter__ = 5

    def __exit__(self, typ, val, tb):
        return None


caught = None
try:
    with IntEnter():
        pass
except TypeError as e:
    caught = str(e)
assert caught == "'int' object is not callable"


# === Lambda dunders work (any callable class member), including on
# classes created dynamically via 3-arg type() ===
DynCm = type('DynCm', (), {'__enter__': lambda self: 'dyn', '__exit__': lambda self, typ, val, tb: None})
with DynCm() as v:
    assert v == 'dyn'

# === Direct __enter__() / __exit__() calls are ordinary method calls ===
p = Passthrough()
assert p.__enter__() is p
assert p.__exit__(None, None, None) is None
assert Grab().__exit__(ValueError, ValueError('x'), None) is True


# The `with` statement looks dunders up on the CLASS only; a direct call
# reads through the instance and so sees an instance attribute shadowing it.
class Shadow:
    def __enter__(self):
        return 'class-enter'

    def __exit__(self, typ, val, tb):
        return None


def instance_enter():
    return 'instance-enter'


s = Shadow()
s.__enter__ = instance_enter
with s as v:
    assert v == 'class-enter'
assert s.__enter__() == 'instance-enter'

# === Unpack failure inside the `with` still invokes __exit__ ===
# `Passthrough.__enter__` returns self, which is not iterable, so the
# `as (a, b)` unpack raises; the unpack lives inside the protected region,
# so __exit__ runs — here it raises its own error which replaces the
# in-flight TypeError, proving it was called.
caught = None
try:
    with BadExit() as (a, b):
        pass
except ValueError as e:
    caught = str(e)
except TypeError:
    caught = 'unpack-error-uncaught'
assert caught == 'cleanup-failed'

# === Early exits (return/break/continue) call __exit__ ===
log = []


def early_return():
    with Tracked('ret'):
        return 'early'


assert early_return() == 'early'
assert log == ['enter ret', 'exit ret']

log = []
for i in range(3):
    with Tracked('loop' + str(i)):
        if i == 1:
            continue
        if i == 2:
            break
assert log == [
    'enter loop0',
    'exit loop0',
    'enter loop1',
    'exit loop1',
    'enter loop2',
    'exit loop2',
]

# === Nested and multi-item with: LIFO exit order ===
log = []
with Tracked('outer'), Tracked('inner'):
    log.append('body')
assert log == ['enter outer', 'enter inner', 'body', 'exit inner', 'exit outer']

# Inner suppress prevents the outer manager from seeing the exception.
outcome = None
with Passthrough():
    try:
        with Grab():
            raise ValueError('inner-swallow')
        outcome = 'fell-through'
    except ValueError:
        outcome = 'propagated'
assert outcome == 'fell-through'

# Outer __exit__ raising while an inner exception propagates replaces it.
caught_info = None
try:
    with BadExit():
        with Passthrough():
            raise RuntimeError('inner-raise')
except ValueError as e:
    caught_info = ('ValueError', str(e))
except RuntimeError as e:
    caught_info = ('RuntimeError', str(e))
assert caught_info == ('ValueError', 'cleanup-failed')
