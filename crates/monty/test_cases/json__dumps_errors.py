import json

# === allow_nan=False errors ===
try:
    json.dumps(float('inf'), allow_nan=False)
    assert False, 'should raise ValueError for inf'
except ValueError as exc:
    assert str(exc) == 'Out of range float values are not JSON compliant: inf'

try:
    json.dumps(float('-inf'), allow_nan=False)
    assert False, 'should raise ValueError for -inf'
except ValueError as exc:
    assert str(exc) == 'Out of range float values are not JSON compliant: -inf'

# === not JSON serializable errors ===
try:
    json.dumps({1})
    assert False, 'set should not be serializable'
except TypeError as exc:
    assert str(exc) == 'Object of type set is not JSON serializable'

# === separators errors ===
try:
    json.dumps(1, separators=[',', ':', 'x'])
    assert False, 'list of 3 separators should raise ValueError'
except ValueError as exc:
    assert str(exc) == 'too many values to unpack (expected 2, got 3)'

try:
    json.dumps(1, separators=[','])
    assert False, 'list of 1 separator should raise ValueError'
except ValueError as exc:
    assert str(exc) == 'not enough values to unpack (expected 2, got 1)'

try:
    json.dumps(1, separators=42)
    assert False, 'int separators should raise TypeError'
except TypeError as exc:
    assert str(exc) == 'cannot unpack non-iterable int object'

try:
    json.dumps(1, separators=[1, ':'])
    assert False, 'non-string first separator should raise TypeError'
except TypeError as exc:
    assert str(exc) == 'make_encoder() argument 6 must be str, not int'

try:
    json.dumps(1, separators=[',', 2])
    assert False, 'non-string second separator should raise TypeError'
except TypeError as exc:
    assert str(exc) == 'make_encoder() argument 5 must be str, not int'

# === unexpected keyword argument ===
try:
    json.dumps(None, foobar_not_static=True)
    assert False, 'unexpected kwarg should raise TypeError'
except TypeError as exc:
    assert str(exc) == "JSONEncoder.__init__() got an unexpected keyword argument 'foobar_not_static'"

try:
    json.dumps(1, 2)
    assert False, 'json.dumps with too many positional args should raise TypeError'
except TypeError as exc:
    assert str(exc) == 'dumps() takes 1 positional argument but 2 were given', f'dumps-arity: {exc}'

# the keyword-only suffix composes into the arity message like CPython
try:
    json.dumps(1, 2, skipkeys=True)
    assert False, 'json.dumps with extra positional + kwonly should raise TypeError'
except TypeError as exc:
    assert (
        str(exc)
        == 'dumps() takes 1 positional argument but 2 positional arguments (and 1 keyword-only argument) were given'
    ), f'dumps-kwonly-arity: {exc}'

# === circular reference errors ===
circular_list = []
circular_list.append(circular_list)
try:
    json.dumps(circular_list)
    assert False, 'circular list should raise ValueError'
except ValueError as exc:
    assert str(exc) == 'Circular reference detected'

circular_dict = {}
circular_dict['self'] = circular_dict
try:
    json.dumps(circular_dict)
    assert False, 'circular dict should raise ValueError'
except ValueError as exc:
    assert str(exc) == 'Circular reference detected'

# === nested circular reference ===
outer = []
inner = [outer]
outer.append(inner)
try:
    json.dumps(outer)
    assert False, 'nested circular should raise ValueError'
except ValueError as exc:
    assert str(exc) == 'Circular reference detected'

# === circular reference in dict value ===
d = {}
d['a'] = [d]
try:
    json.dumps(d)
    assert False, 'circular dict in list should raise ValueError'
except ValueError as exc:
    assert str(exc) == 'Circular reference detected'

# === allow_nan=False with float dict keys ===
try:
    json.dumps({float('nan'): 1}, allow_nan=False)
    assert False, 'should raise ValueError for NaN key'
except ValueError as exc:
    assert str(exc) == 'Out of range float values are not JSON compliant: nan'

try:
    json.dumps({float('inf'): 1}, allow_nan=False)
    assert False, 'should raise ValueError for inf key'
except ValueError as exc:
    assert str(exc) == 'Out of range float values are not JSON compliant: inf'

try:
    json.dumps({float('-inf'): 1}, allow_nan=False)
    assert False, 'should raise ValueError for -inf key'
except ValueError as exc:
    assert str(exc) == 'Out of range float values are not JSON compliant: -inf'
