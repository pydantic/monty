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

# === skipkeys preserves insertion order ===
d = {}
d['a'] = 1
d[(1,)] = 'skip'
d['b'] = 2
d[(2,)] = 'skip'
d['c'] = 3
assert json.dumps(d, skipkeys=True) == '{"a": 1, "b": 2, "c": 3}', 'skipkeys preserves order of remaining keys'

# === skipkeys drops keys at the start ===
d2 = {}
d2[(1,)] = 'skip'
d2['a'] = 1
d2['b'] = 2
assert json.dumps(d2, skipkeys=True) == '{"a": 1, "b": 2}', 'skipkeys drops disallowed keys at the start'

# === skipkeys drops keys at the end ===
d3 = {}
d3['a'] = 1
d3['b'] = 2
d3[(1,)] = 'skip'
assert json.dumps(d3, skipkeys=True) == '{"a": 1, "b": 2}', 'skipkeys drops disallowed keys at the end'

# === skipkeys drops consecutive disallowed keys ===
d4 = {}
d4['a'] = 1
d4[(1,)] = 'skip'
d4[(2,)] = 'skip'
d4[(3,)] = 'skip'
d4['b'] = 2
assert json.dumps(d4, skipkeys=True) == '{"a": 1, "b": 2}', 'skipkeys drops consecutive disallowed keys'

# === skipkeys with single allowed key ===
assert json.dumps({(1,): 'skip', 'a': 1, (2,): 'skip'}, skipkeys=True) == '{"a": 1}', (
    'skipkeys with only one allowed key'
)

# === skipkeys with single disallowed key ===
assert json.dumps({(1,): 1}, skipkeys=True) == '{}', 'skipkeys with single disallowed key produces empty object'

# === skipkeys with only allowed keys is a no-op ===
assert json.dumps({'a': 1, 'b': 2}, skipkeys=True) == '{"a": 1, "b": 2}', (
    'skipkeys with all allowed keys changes nothing'
)

# === skipkeys in nested dicts ===
assert json.dumps({'a': {(1,): 'skip', 'b': 2}}, skipkeys=True) == '{"a": {"b": 2}}', (
    'skipkeys applies recursively to nested dicts'
)
assert json.dumps({'a': {(1,): 'skip', (2,): 'skip'}}, skipkeys=True) == '{"a": {}}', (
    'skipkeys recursively produces empty nested dict'
)

# === skipkeys with complex values on skipped entries ===
assert (
    json.dumps(
        {
            (1,): [1, 2, 3],
            'a': {'nested': True},
            (2,): {'also': 'skipped'},
            'b': [4, 5],
        },
        skipkeys=True,
    )
    == '{"a": {"nested": true}, "b": [4, 5]}'
), 'skipkeys drops entries with complex values without errors'

# === skipkeys combined with indent ===
result = json.dumps({'a': 1, (1,): 'skip', 'b': 2}, skipkeys=True, indent=2)
assert result == '{\n  "a": 1,\n  "b": 2\n}', 'skipkeys works with indent'

# === skipkeys with mixed allowed key types ===
assert (
    json.dumps(
        {
            'str': 1,
            42: 2,
            True: 3,
            None: 4,
            3.14: 5,
            (1,): 'skip',
        },
        skipkeys=True,
    )
    == '{"str": 1, "42": 2, "true": 3, "null": 4, "3.14": 5}'
), 'skipkeys drops tuple but keeps str, int, bool, None, float keys'

# === skipkeys with bytes key ===
assert json.dumps({b'hello': 1, 'a': 2}, skipkeys=True) == '{"a": 2}', 'skipkeys drops bytes keys'

# === skipkeys with empty dict ===
assert json.dumps({}, skipkeys=True) == '{}', 'skipkeys with empty dict'

# === skipkeys=False TypeError for various disallowed types ===
try:
    json.dumps({(1, 2): 3})
    assert False, 'should raise TypeError for tuple key without skipkeys'
except TypeError as exc:
    assert str(exc) == 'keys must be str, int, float, bool or None, not tuple', 'TypeError message for tuple key'

try:
    json.dumps({b'hello': 1})
    assert False, 'should raise TypeError for bytes key without skipkeys'
except TypeError as exc:
    assert str(exc) == 'keys must be str, int, float, bool or None, not bytes', 'TypeError message for bytes key'

# === skipkeys + sort_keys (only allowed keys) ===
d5 = {}
d5['c'] = 3
d5['a'] = 1
d5['b'] = 2
assert json.dumps(d5, skipkeys=True, sort_keys=True) == '{"a": 1, "b": 2, "c": 3}', (
    'skipkeys + sort_keys: allowed keys are sorted correctly'
)

# === string key with ensure_ascii ===
assert json.dumps({'hello': 1}) == '{"hello": 1}', 'simple string key'
