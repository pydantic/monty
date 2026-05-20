# Calling a non-callable value must raise `TypeError: '<type>' object is not callable`,
# matching CPython. Exercises the fallback arm of `call_heap_callable` for heap-typed
# values (list, dict, set, tuple, frozenset, bytes) and the non-heap arm of
# `call_function` for primitive values (int, float, bool, None, str).

# === heap-typed non-callables ===

try:
    [1, 2, 3]()
    assert False, 'expected list call to raise'
except TypeError as e:
    assert str(e) == "'list' object is not callable", f'list: {e}'

try:
    {'k': 1}()
    assert False, 'expected dict call to raise'
except TypeError as e:
    assert str(e) == "'dict' object is not callable", f'dict: {e}'

try:
    {1, 2}()
    assert False, 'expected set call to raise'
except TypeError as e:
    assert str(e) == "'set' object is not callable", f'set: {e}'

try:
    (1, 2)()
    assert False, 'expected tuple call to raise'
except TypeError as e:
    assert str(e) == "'tuple' object is not callable", f'tuple: {e}'

try:
    frozenset([1, 2])()
    assert False, 'expected frozenset call to raise'
except TypeError as e:
    assert str(e) == "'frozenset' object is not callable", f'frozenset: {e}'

try:
    b'abc'()
    assert False, 'expected bytes call to raise'
except TypeError as e:
    assert str(e) == "'bytes' object is not callable", f'bytes: {e}'

# === primitive (non-heap) non-callables ===

try:
    (42)()
    assert False, 'expected int call to raise'
except TypeError as e:
    assert str(e) == "'int' object is not callable", f'int: {e}'

try:
    (3.14)()
    assert False, 'expected float call to raise'
except TypeError as e:
    assert str(e) == "'float' object is not callable", f'float: {e}'

try:
    True()
    assert False, 'expected bool call to raise'
except TypeError as e:
    assert str(e) == "'bool' object is not callable", f'bool: {e}'

try:
    None()
    assert False, 'expected None call to raise'
except TypeError as e:
    assert str(e) == "'NoneType' object is not callable", f'None: {e}'

# === non-callable bound to a variable, then called ===
# Exercises the path the dataclass field-call fix in #352 routes through:
# load value, then call. Same error must result.

x = [1, 2, 3]
try:
    x()
    assert False, 'expected variable-bound list call to raise'
except TypeError as e:
    assert str(e) == "'list' object is not callable", f'var list: {e}'
