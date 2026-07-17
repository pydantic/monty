import json

# === loads basics ===
assert json.loads('null') is None
assert json.loads('true') is True
assert json.loads('false') is False
assert json.loads('123') == 123
assert json.loads('1.5') == 1.5
assert json.loads('"hello"') == 'hello'
assert json.loads('[1, 2, 3]') == [1, 2, 3]
assert json.loads('{"a": 1, "b": [true, null]}') == {
    'a': 1,
    'b': [True, None],
}

# === loads bytes and unicode ===
assert json.loads(b'{"a":[1,true,null]}') == {'a': [1, True, None]}
assert json.loads(b'123') == 123
assert json.loads(b'1.5') == 1.5
assert json.loads(b'"hello"') == 'hello'
assert json.loads(b'[1, 2, 3]') == [1, 2, 3]
assert json.loads(b'true') is True
assert json.loads(b'false') is False
assert json.loads(b'null') is None
assert json.loads('"\\u2603"') == '☃'
assert json.loads('{"a": 1, "a": 2}') == {'a': 2}

# === loads big integers ===
big = 1234567890123456789012345678901234567890
assert json.loads(str(big)) == big

# === dumps basics ===
assert json.dumps(None) == 'null'
assert json.dumps(True) == 'true'
assert json.dumps(False) == 'false'
assert json.dumps(123) == '123'
assert json.dumps(1.0) == '1.0'
assert json.dumps(1.5) == '1.5'
assert json.dumps('hello') == '"hello"'
assert json.dumps([1, 2, 3]) == '[1, 2, 3]'
assert json.dumps({'a': 1}) == '{"a": 1}'
assert json.dumps((1, 2, 3)) == '[1, 2, 3]'

# === dumps formatting ===
assert json.dumps({'b': 1, 'a': 2}, sort_keys=True) == ('{"a": 2, "b": 1}')
assert json.dumps({'a': [1, 2]}, indent=2) == ('{\n  "a": [\n    1,\n    2\n  ]\n}')
assert json.dumps({'a': [1, 2]}, indent='--') == ('{\n--"a": [\n----1,\n----2\n--]\n}')
assert json.dumps({'a': 1}, separators=(',', ':')) == '{"a":1}'

# === dumps unicode and floats ===
assert json.dumps('☃') == '"\\u2603"'
assert json.dumps('☃', ensure_ascii=False) == ('"☃"')
assert json.dumps(float('nan')) == 'NaN'
assert json.dumps(float('inf')) == ('Infinity')
assert json.dumps(float('-inf')) == ('-Infinity')

# === dumps key coercion ===
assert json.dumps({True: 1, False: 2, None: 3, 4: 5, 1.5: 6}) == '{"true": 1, "false": 2, "null": 3, "4": 5, "1.5": 6}'

# === dumps skipkeys ===
assert json.dumps({(1, 2): 3}, skipkeys=True) == '{}'

# === empty containers ===
assert json.dumps([]) == '[]'
assert json.dumps({}) == '{}'
assert json.loads('[]') == []
assert json.loads('{}') == {}

# === roundtrip ===
data = {'a': [1, 2.5, True, None, 'x', {'b': [3]}]}
assert json.loads(json.dumps(data)) == data

# === JSONDecodeError subclassing ===
try:
    json.loads('{]')
    assert False, 'invalid JSON should raise JSONDecodeError'
except json.JSONDecodeError as exc:
    assert str(exc) == 'Expecting property name enclosed in double quotes: line 1 column 2 (char 1)'

caught_value_error = False
try:
    json.loads('{]')
except ValueError:
    caught_value_error = True
assert caught_value_error

# === dumps error handling ===
try:
    json.dumps(float('nan'), allow_nan=False)
    assert False, 'allow_nan=False should reject NaN'
except ValueError as exc:
    assert str(exc) == 'Out of range float values are not JSON compliant: nan'

try:
    json.dumps({(1, 2): 3})
    assert False, 'unsupported dict key type should raise TypeError'
except TypeError as exc:
    assert str(exc) == 'keys must be str, int, float, bool or None, not tuple'

try:
    json.dumps({1})
    assert False, 'set should not be JSON serializable'
except TypeError as exc:
    assert str(exc) == 'Object of type set is not JSON serializable'

try:
    json.loads(1)
    assert False, 'loads(int) should raise TypeError'
except TypeError as exc:
    assert str(exc) == 'the JSON object must be str, bytes or bytearray, not int'

# === circular reference detection ===
circular = []
circular.append(circular)
try:
    json.dumps(circular)
    assert False, 'circular reference should raise ValueError'
except ValueError as exc:
    assert str(exc) == 'Circular reference detected'
