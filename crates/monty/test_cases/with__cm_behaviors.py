# Context-manager behaviors exercised via the `_test_cm()` synthetic context
# manager. `_test_cm()` is a test-only hook injected on both sides — the
# Monty side enables it via the `test-hooks` cargo feature, the CPython
# side via `scripts/test_cm_shim.py`. These tests verify branches of the
# `with` machinery that `OpenFile` (Monty's only production context
# manager) doesn't cover today.

# === Passthrough: returns self on enter, propagates exceptions ===
cm = _test_cm()
with cm as bound:
    assert bound is cm, 'default __enter__ returns self'
caught = None
try:
    with _test_cm():
        raise ValueError('passthrough-prop')
except ValueError as e:
    caught = str(e)
assert caught == 'passthrough-prop', 'default __exit__ does not suppress'

# === Suppress: __exit__ returns True, swallows in-flight exception ===
swallowed = False
with _test_cm('suppress'):
    raise ValueError('to-swallow')
    swallowed = False  # unreachable
swallowed = True
assert swallowed, 'suppress branch lets control fall through to after the with'

# Suppress does NOT swallow when there is no in-flight exception: a normal
# body completes normally either way.
with _test_cm('suppress'):
    inside = 'ran'
assert inside == 'ran', 'normal-exit path is unaffected by the suppress flag'

# === enter_value: __enter__ returns a non-self value ===
with _test_cm('enter_value', 42) as v:
    assert v == 42, 'as-target gets __enter__ return value'

# === raise_on_enter: body never runs ===
ran_body = False
caught = None
try:
    with _test_cm('raise_on_enter', 'no-entry'):
        ran_body = True
except ValueError as e:
    caught = str(e)
assert caught == 'no-entry', '__enter__ exception propagates'
assert not ran_body, 'body skipped when __enter__ raises'

# === raise_on_exit: on normal-exit path replaces (nonexistent) exception ===
caught = None
try:
    with _test_cm('raise_on_exit', 'cleanup-failed'):
        pass
except ValueError as e:
    caught = str(e)
assert caught == 'cleanup-failed', '__exit__ exception propagates after normal body'

# === raise_on_exit: on exception path REPLACES the in-flight exception ===
# CPython semantics: an exception raised by __exit__ replaces the one
# already propagating; the original is lost (not chained as __context__
# in Monty, which doesn't track exception chaining yet).
caught_type = None
caught_msg = None
try:
    with _test_cm('raise_on_exit', 'cleanup-wins'):
        raise RuntimeError('original')
except ValueError as e:
    caught_type = 'ValueError'
    caught_msg = str(e)
except RuntimeError:
    caught_type = 'RuntimeError'
assert caught_type == 'ValueError', '__exit__ exception replaces in-flight RuntimeError'
assert caught_msg == 'cleanup-wins', 'replacing exception carries its own message'

# === Direct __enter__() / __exit__() invocation ===
cm = _test_cm()
assert cm.__enter__() is cm, 'direct __enter__() works and returns self'
assert cm.__exit__(None, None, None) is None, 'direct __exit__() returns None'

# Suppress flag is exception-path-only — direct invocation with a None
# value (no in-flight exception) returns None.
assert _test_cm('suppress').__exit__(None, None, None) is None, (
    'direct __exit__(None, None, None) ignores the suppress flag'
)

# Forwarding a real exception instance to a `suppress`-configured manager
# *should* trip the suppress branch and return True. Verifies that the
# `val` argument of __exit__ is forwarded to py_exit, not silently dropped.
assert _test_cm('suppress').__exit__(ValueError, ValueError('x'), None) is True, (
    'direct __exit__ with an exception value routes to the suppress branch'
)

# Wrong arity — `__exit__` requires exactly 3 positional arguments.
err = None
try:
    _test_cm().__exit__()
except TypeError as e:
    err = str(e)
assert err is not None, '__exit__() with zero args should raise TypeError'

err = None
try:
    _test_cm().__exit__(None, None, None, None)
except TypeError as e:
    err = str(e)
assert err is not None, '__exit__() with four args should raise TypeError'

# === Unpack failure inside the `with` body still invokes __exit__ ===
# `TestContextManager` is not iterable, so `as (a, b):` fails during unpack.
# The unpack lives inside the protected region, so __exit__ runs and its
# ValueError replaces the in-flight TypeError.
caught = None
try:
    with _test_cm('raise_on_exit', 'cleanup-from-unpack-fail') as (a, b):
        pass
except ValueError as e:
    caught = str(e)
except TypeError:
    caught = 'unpack-error-uncaught'
assert caught == 'cleanup-from-unpack-fail', '__exit__ runs when unpacking the as-target fails'

# === Multi-item with: suppress on inner manager only swallows in inner scope ===
# Outer manager sees the exception (because it's propagating after the
# inner one swallowed nothing, since the inner doesn't suppress here).
outer_saw = None
with _test_cm() as _a:
    try:
        with _test_cm('suppress') as _b:
            raise ValueError('inner-swallow')
        outer_saw = 'fell-through'
    except ValueError:
        outer_saw = 'propagated'
assert outer_saw == 'fell-through', 'inner suppress prevents outer from seeing the exception'

# === Nested raise_on_exit: outer __exit__ raising during exception path ===
caught_type = None
try:
    with _test_cm('raise_on_exit', 'outer-fails'):
        with _test_cm():
            raise RuntimeError('inner-raise')
except ValueError as e:
    caught_type = ('ValueError', str(e))
except RuntimeError as e:
    caught_type = ('RuntimeError', str(e))
assert caught_type == ('ValueError', 'outer-fails'), 'outer __exit__ raising replaces inner exception'
