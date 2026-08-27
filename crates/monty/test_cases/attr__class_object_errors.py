# An unknown attribute on a builtin *type object* names the class, not the
# metaclass: `type object 'list' has no attribute 'nonexistent'`, never
# `'type' object has no attribute 'nonexistent'`.
import datetime

_classes = [
    (int, 'int'),
    (str, 'str'),
    (list, 'list'),
    (dict, 'dict'),
    (tuple, 'tuple'),
    (set, 'set'),
    (frozenset, 'frozenset'),
    (bytes, 'bytes'),
    (range, 'range'),
    (datetime.datetime, 'datetime.datetime'),
]

# === attribute access ===
for _cls, _name in _classes:
    try:
        _cls.nonexistent
        assert False, 'expected AttributeError'
    except AttributeError as e:
        assert str(e) == f"type object '{_name}' has no attribute 'nonexistent'"

# === calling an unknown class method takes the same wording ===
for _cls, _name in _classes:
    try:
        _cls.nonexistent()
        assert False, 'expected AttributeError'
    except AttributeError as e:
        assert str(e) == f"type object '{_name}' has no attribute 'nonexistent'"

# === instances still report the instance form ===
try:
    [1].nonexistent
    assert False, 'expected AttributeError'
except AttributeError as e:
    assert str(e) == "'list' object has no attribute 'nonexistent'"

# === a known class method is unaffected ===
assert dict.fromkeys(['a'], 1) == {'a': 1}
assert bytes.fromhex('ff') == b'\xff'
