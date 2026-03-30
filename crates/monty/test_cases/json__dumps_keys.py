import json

# === float special value keys ===
assert json.dumps({float('nan'): 1}) == '{"NaN": 1}', 'NaN float key is coerced to "NaN"'
assert json.dumps({float('inf'): 1}) == '{"Infinity": 1}', 'inf float key is coerced to "Infinity"'
assert json.dumps({float('-inf'): 1}) == '{"-Infinity": 1}', '-inf float key is coerced to "-Infinity"'

# === bigint key ===
big = 10**40
assert json.dumps({big: 1}) == '{"10000000000000000000000000000000000000000": 1}', (
    'big integer keys are coerced to decimal strings'
)

# === skipkeys with various unsupported key types ===
assert json.dumps({(1, 2): 'a', 'b': 'c'}, skipkeys=True) == '{"b": "c"}', (
    'skipkeys drops tuple keys and keeps string keys'
)
assert json.dumps({(1,): 1, (2,): 2}, skipkeys=True) == '{}', 'skipkeys drops all unsupported keys leaving empty dict'

# === skipkeys=False (default) error ===
try:
    json.dumps({(1, 2): 3})
    assert False, 'should raise TypeError for tuple key'
except TypeError as exc:
    assert str(exc) == 'keys must be str, int, float, bool or None, not tuple', (
        'invalid key type error message for tuple'
    )

# === sort_keys error with mixed types ===
try:
    json.dumps({1: 'a', 'b': 'c'}, sort_keys=True)
    assert False, 'sort_keys with mixed key types should raise TypeError'
except TypeError as exc:
    assert "'<' not supported between instances of 'str' and 'int'" == str(exc), (
        'sort_keys mixed types error matches CPython'
    )

# === all allowed key types together ===
result = json.dumps({True: 1, False: 2, None: 3, 4: 5, 1.5: 6, 'a': 7})
assert result == '{"true": 1, "false": 2, "null": 3, "4": 5, "1.5": 6, "a": 7}', (
    'all allowed key types serialize correctly'
)

# === string key with ensure_ascii ===
assert json.dumps({'hello': 1}) == '{"hello": 1}', 'simple string key'
