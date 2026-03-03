import json
from json import dumps, loads

# === dumps: primitives ===
assert json.dumps(None) == 'null', 'None should serialize to null'
assert json.dumps(True) == 'true', 'True should serialize to true'
assert json.dumps(False) == 'false', 'False should serialize to false'
assert json.dumps(123) == '123', 'int should serialize as a JSON number'
assert json.dumps(1.25) == '1.25', 'float should serialize as a JSON number'
assert json.dumps(1.0) == '1.0', 'whole-number float should preserve .0'
assert json.dumps(-0.0) == '-0.0', 'negative zero float should preserve sign'
assert json.dumps(float('nan')) == 'NaN', 'NaN should match CPython default encoding'
assert json.dumps(float('inf')) == 'Infinity', 'Infinity should match CPython default encoding'
assert json.dumps(float('-inf')) == '-Infinity', 'negative Infinity should match CPython default encoding'
assert json.dumps('hello') == '"hello"', 'str should be quoted JSON string'
assert json.dumps('é') == '"\\u00e9"', 'dumps should use ensure_ascii=True behavior'

# === dumps: containers ===
assert json.dumps([1, 2, 3]) == '[1, 2, 3]', 'list should serialize to array'
assert json.dumps((1, 2, 3)) == '[1, 2, 3]', 'tuple should serialize to array'
assert json.dumps({'a': 1, 'b': [True, None]}) == '{"a": 1, "b": [true, null]}', 'nested values should serialize'

# === dumps: dict key coercion ===
assert json.dumps({1: 'a', 2: 'b'}) == '{"1": "a", "2": "b"}', 'int keys should be coerced to strings'
assert json.dumps({1.5: 'a'}) == '{"1.5": "a"}', 'float keys should be coerced to strings'
assert json.dumps({True: 1, False: 2}) == '{"true": 1, "false": 2}', 'bool keys should use JSON names'
assert json.dumps({None: 'x'}) == '{"null": "x"}', 'None keys should map to "null"'

# === loads: primitives and containers ===
assert json.loads('null') is None, 'null should parse to None'
assert json.loads('true') is True, 'true should parse to True'
assert json.loads('false') is False, 'false should parse to False'
assert json.loads('123') == 123, 'integer should parse to int'
assert json.loads('1.25') == 1.25, 'float should parse to float'
assert json.loads('"\\u00e9"') == 'é', 'unicode escape should decode to unicode char'
assert json.loads('[1, 2, 3]') == [1, 2, 3], 'array should parse to list'
assert json.loads('{"a": 1, "b": [true, null]}') == {'a': 1, 'b': [True, None]}, 'object should parse to dict'
assert json.loads(b'{"x": 1}') == {'x': 1}, 'bytes input should parse as UTF-8 JSON text'

# === import forms ===
assert dumps({'x': 1}) == '{"x": 1}', 'from-import dumps should resolve function correctly'
assert loads('{"x": 1}') == {'x': 1}, 'from-import loads should resolve function correctly'

# === dumps: error cases ===
try:
    json.dumps({(1, 2): 'bad'})
    assert False, 'tuple dict key should raise TypeError'
except TypeError as exc:
    assert exc.args[0] == 'keys must be str, int, float, bool or None, not tuple', (
        'tuple dict key error should match CPython message'
    )

try:
    json.dumps(set([1, 2]))
    assert False, 'set should raise TypeError'
except TypeError as exc:
    assert exc.args[0] == 'Object of type set is not JSON serializable', 'set error should match CPython message'

cycle = []
cycle.append(cycle)
try:
    json.dumps(cycle)
    assert False, 'circular list should raise ValueError'
except ValueError as exc:
    assert exc.args[0] == 'Circular reference detected', 'circular list error should match CPython message'

# === loads: error cases ===
try:
    json.loads('{')
    assert False, 'invalid JSON should raise ValueError subclass'
except ValueError as exc:
    assert isinstance(exc, ValueError), 'loads decode failures should be ValueError-compatible'
    assert len(exc.args) == 1, 'decode error should include a single message argument'

try:
    json.loads(1)
    assert False, 'non-str/bytes input should raise TypeError'
except TypeError as exc:
    assert exc.args[0] == 'the JSON object must be str, bytes or bytearray, not int', (
        'loads input type error should match CPython message'
    )
