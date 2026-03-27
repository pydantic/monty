import json

# === indent edge cases ===
assert json.dumps([1, 2], indent=False) == '[\n1,\n2\n]', 'indent=False behaves like indent=0'
assert json.dumps({'a': 1}, indent=False) == '{\n"a": 1\n}', 'indent=False on dict uses newline-only'

# === empty containers with indent ===
assert json.dumps([], indent=2) == '[]', 'empty list with indent stays compact'
assert json.dumps({}, indent=2) == '{}', 'empty dict with indent stays compact'
assert json.dumps((), indent=2) == '[]', 'empty tuple with indent stays compact'

# === separators=None preserves indent-aware defaults ===
assert json.dumps({'a': [1, 2]}, indent=2, separators=None) == json.dumps({'a': [1, 2]}, indent=2), (
    'separators=None with indent uses same defaults as omitting separators'
)

# === separators as tuple ===
assert json.dumps({'a': 1}, separators=(',', ':')) == '{"a":1}', 'tuple separators work'
assert json.dumps([1, 2], separators=(' , ', ' : ')) == '[1 , 2]', 'custom separators with spaces'

# === separators as list ===
assert json.dumps({'a': 1}, separators=[',', ':']) == '{"a":1}', 'list separators work'

# === indent with explicit separators ===
assert json.dumps({'a': 1}, indent=2, separators=(',', ': ')) == '{\n  "a": 1\n}', (
    'indent with explicit separators does not override to defaults'
)

# === nested structures with indent ===
assert json.dumps({'a': {'b': 1}}, indent=2) == '{\n  "a": {\n    "b": 1\n  }\n}', (
    'nested dict pretty prints with proper nesting'
)
assert json.dumps({'a': [1, 2], 'b': {'c': 3}}, indent=2) == (
    '{\n  "a": [\n    1,\n    2\n  ],\n  "b": {\n    "c": 3\n  }\n}'
), 'mixed nested containers with indent'

# === empty inner containers with indent ===
assert json.dumps({'a': [], 'b': {}}, indent=2) == '{\n  "a": [],\n  "b": {}\n}', (
    'empty inner containers stay compact even with indent'
)

# === deeply nested structures ===
assert json.dumps([[[[1]]]]) == '[[[[1]]]]', 'deeply nested lists serialize'
assert json.dumps({'a': {'b': {'c': {'d': 1}}}}) == '{"a": {"b": {"c": {"d": 1}}}}', 'deeply nested dicts serialize'

# === sort_keys with multiple keys ===
assert json.dumps({'c': 3, 'a': 1, 'b': 2}, sort_keys=True) == '{"a": 1, "b": 2, "c": 3}', (
    'sort_keys sorts string keys alphabetically'
)
assert json.dumps({}, sort_keys=True) == '{}', 'sort_keys on empty dict'
assert json.dumps({'z': 1}, sort_keys=True) == '{"z": 1}', 'sort_keys on single key'

# === sort_keys with indent ===
assert json.dumps({'b': 1, 'a': 2}, sort_keys=True, indent=2) == ('{\n  "a": 2,\n  "b": 1\n}'), (
    'sort_keys combined with indent'
)

# === multiple refs to same object (not circular) ===
shared = [1, 2]
assert json.dumps([shared, shared]) == '[[1, 2], [1, 2]]', (
    'multiple references to same list are not flagged as circular'
)

# === long integers beyond i64 range ===
big = 2**63 + 1
assert json.dumps(big) == '9223372036854775809', 'long int above i64::MAX serializes'
assert json.dumps(-big) == '-9223372036854775809', 'negative long int below i64::MIN serializes'

# === string escaping ===
assert json.dumps('a\\b') == '"a\\\\b"', 'backslash is escaped'
assert json.dumps('a"b') == '"a\\"b"', 'double quote is escaped'
assert json.dumps('\x00') == '"\\u0000"', 'null byte is escaped'
assert json.dumps('\x01') == '"\\u0001"', 'control char 0x01 is escaped'
assert json.dumps('\x1f') == '"\\u001f"', 'control char 0x1f is escaped'
assert json.dumps('\x7f') == '"\\u007f"', 'DEL char 0x7f is escaped with ensure_ascii'
assert json.dumps('\x7f', ensure_ascii=False) == '"\x7f"', 'DEL char 0x7f is literal with ensure_ascii=False'
