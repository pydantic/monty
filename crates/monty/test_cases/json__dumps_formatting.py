import json

# === indent edge cases ===
assert json.dumps([1, 2], indent=False) == '[\n1,\n2\n]'
assert json.dumps({'a': 1}, indent=False) == '{\n"a": 1\n}'

# === empty containers with indent ===
assert json.dumps([], indent=2) == '[]'
assert json.dumps({}, indent=2) == '{}'
assert json.dumps((), indent=2) == '[]'

# === separators=None preserves indent-aware defaults ===
assert json.dumps({'a': [1, 2]}, indent=2, separators=None) == json.dumps({'a': [1, 2]}, indent=2)

# === separators as tuple ===
assert json.dumps({'a': 1}, separators=(',', ':')) == '{"a":1}'
assert json.dumps([1, 2], separators=(' , ', ' : ')) == '[1 , 2]'

# === separators as list ===
assert json.dumps({'a': 1}, separators=[',', ':']) == '{"a":1}'

# === indent with explicit separators ===
assert json.dumps({'a': 1}, indent=2, separators=(',', ': ')) == '{\n  "a": 1\n}'

# === nested structures with indent ===
assert json.dumps({'a': {'b': 1}}, indent=2) == '{\n  "a": {\n    "b": 1\n  }\n}'
assert json.dumps({'a': [1, 2], 'b': {'c': 3}}, indent=2) == (
    '{\n  "a": [\n    1,\n    2\n  ],\n  "b": {\n    "c": 3\n  }\n}'
)

# === empty inner containers with indent ===
assert json.dumps({'a': [], 'b': {}}, indent=2) == '{\n  "a": [],\n  "b": {}\n}'

# === deeply nested structures ===
assert json.dumps([[[[1]]]]) == '[[[[1]]]]'
assert json.dumps({'a': {'b': {'c': {'d': 1}}}}) == '{"a": {"b": {"c": {"d": 1}}}}'

# === sort_keys with multiple keys ===
assert json.dumps({'c': 3, 'a': 1, 'b': 2}, sort_keys=True) == '{"a": 1, "b": 2, "c": 3}'
assert json.dumps({}, sort_keys=True) == '{}'
assert json.dumps({'z': 1}, sort_keys=True) == '{"z": 1}'

# === sort_keys with indent ===
assert json.dumps({'b': 1, 'a': 2}, sort_keys=True, indent=2) == ('{\n  "a": 2,\n  "b": 1\n}')

# === multiple refs to same object (not circular) ===
shared = [1, 2]
assert json.dumps([shared, shared]) == '[[1, 2], [1, 2]]'

# === long integers beyond i64 range ===
big = 2**63 + 1
assert json.dumps(big) == '9223372036854775809'
assert json.dumps(-big) == '-9223372036854775809'

# === string escaping ===
assert json.dumps('a\\b') == '"a\\\\b"'
assert json.dumps('a"b') == '"a\\"b"'
assert json.dumps('\x00') == '"\\u0000"'
assert json.dumps('\x01') == '"\\u0001"'
assert json.dumps('\x1f') == '"\\u001f"'
assert json.dumps('\x7f') == '"\\u007f"'
assert json.dumps('\x7f', ensure_ascii=False) == '"\x7f"'
assert json.dumps('😀') == '"\\ud83d\\ude00"'
assert json.dumps('😀', ensure_ascii=False) == '"😀"'
assert json.dumps('ascii😀"\\\x01z') == '"ascii\\ud83d\\ude00\\"\\\\\\u0001z"'
