import json

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
