import json
import sys

_monty = 'Monty' in sys.version

# === allow_nan=False errors ===
try:
    json.dumps(float('inf'), allow_nan=False)
    assert False, 'should raise ValueError for inf'
except ValueError as exc:
    assert str(exc) == 'Out of range float values are not JSON compliant: inf', 'inf error message'

try:
    json.dumps(float('-inf'), allow_nan=False)
    assert False, 'should raise ValueError for -inf'
except ValueError as exc:
    assert str(exc) == 'Out of range float values are not JSON compliant: -inf', '-inf error message'

# === not JSON serializable errors ===
try:
    json.dumps({1})
    assert False, 'set should not be serializable'
except TypeError as exc:
    assert str(exc) == 'Object of type set is not JSON serializable', 'set error message'

# === separators errors ===
try:
    json.dumps(1, separators=[',', ':', 'x'])
    assert False, 'list of 3 separators should raise ValueError'
except ValueError as exc:
    assert str(exc) == 'too many values to unpack (expected 2, got 3)', 'sep list of 3 error message'

try:
    json.dumps(1, separators=[','])
    assert False, 'list of 1 separator should raise ValueError'
except ValueError as exc:
    assert str(exc) == 'not enough values to unpack (expected 2, got 1)', 'sep list of 1 error message'

try:
    json.dumps(1, separators=42)
    assert False, 'int separators should raise TypeError'
except TypeError as exc:
    assert str(exc) == 'cannot unpack non-iterable int object', 'int separators error message'

try:
    json.dumps(1, separators=[1, ':'])
    assert False, 'non-string first separator should raise TypeError'
except TypeError as exc:
    assert str(exc) == 'make_encoder() argument 6 must be str, not int', 'non-string item_separator error'

try:
    json.dumps(1, separators=[',', 2])
    assert False, 'non-string second separator should raise TypeError'
except TypeError as exc:
    assert str(exc) == 'make_encoder() argument 5 must be str, not int', 'non-string key_separator error'

# === unexpected keyword argument ===
try:
    json.dumps(None, foobar_not_static=True)
    assert False, 'unexpected kwarg should raise TypeError'
except TypeError as exc:
    assert str(exc) == "JSONEncoder.__init__() got an unexpected keyword argument 'foobar_not_static'", (
        'unexpected kwarg error message'
    )

try:
    json.dumps(1, 2)
    assert False, 'json.dumps with too many positional args should raise TypeError'
except TypeError as exc:
    # CPython: "dumps() takes 1 positional argument but 2 were given" — that
    # specific wording is only produced by pure-Python functions; json.dumps
    # is implemented in Python in CPython, so it gets that form. Monty emits
    # `PyArg_UnpackTuple`-style "expected at most" for every Python-style
    # `FromArgs` callsite, which is what CPython would emit if dumps were
    # implemented in C — see limitations/json.md.
    if _monty:
        assert str(exc) == 'dumps expected at most 1 argument, got 2', f'monty-dumps-arity: {exc}'
    else:
        assert str(exc) == 'dumps() takes 1 positional argument but 2 were given', f'cpy-dumps-arity: {exc}'

# === circular reference errors ===
circular_list = []
circular_list.append(circular_list)
try:
    json.dumps(circular_list)
    assert False, 'circular list should raise ValueError'
except ValueError as exc:
    assert str(exc) == 'Circular reference detected', 'circular list error'

circular_dict = {}
circular_dict['self'] = circular_dict
try:
    json.dumps(circular_dict)
    assert False, 'circular dict should raise ValueError'
except ValueError as exc:
    assert str(exc) == 'Circular reference detected', 'circular dict error'

# === nested circular reference ===
outer = []
inner = [outer]
outer.append(inner)
try:
    json.dumps(outer)
    assert False, 'nested circular should raise ValueError'
except ValueError as exc:
    assert str(exc) == 'Circular reference detected', 'nested circular error'

# === circular reference in dict value ===
d = {}
d['a'] = [d]
try:
    json.dumps(d)
    assert False, 'circular dict in list should raise ValueError'
except ValueError as exc:
    assert str(exc) == 'Circular reference detected', 'circular dict-in-list error'

# === allow_nan=False with float dict keys ===
try:
    json.dumps({float('nan'): 1}, allow_nan=False)
    assert False, 'should raise ValueError for NaN key'
except ValueError as exc:
    assert str(exc) == 'Out of range float values are not JSON compliant: nan', 'NaN key error message'

try:
    json.dumps({float('inf'): 1}, allow_nan=False)
    assert False, 'should raise ValueError for inf key'
except ValueError as exc:
    assert str(exc) == 'Out of range float values are not JSON compliant: inf', 'inf key error message'

try:
    json.dumps({float('-inf'): 1}, allow_nan=False)
    assert False, 'should raise ValueError for -inf key'
except ValueError as exc:
    assert str(exc) == 'Out of range float values are not JSON compliant: -inf', '-inf key error message'
