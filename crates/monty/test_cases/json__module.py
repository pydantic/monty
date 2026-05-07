import json

# === loads basics ===
assert json.loads('null') is None, 'loads null -> None'
assert json.loads('true') is True, 'loads true -> True'
assert json.loads('false') is False, 'loads false -> False'
assert json.loads('123') == 123, 'loads int'
assert json.loads('1.5') == 1.5, 'loads float'
assert json.loads('"hello"') == 'hello', 'loads string'
assert json.loads('[1, 2, 3]') == [1, 2, 3], 'loads array'
assert json.loads('{"a": 1, "b": [true, null]}') == {
    'a': 1,
    'b': [True, None],
}, 'loads nested object'

# === loads bytes and unicode ===
assert json.loads(b'{"a":[1,true,null]}') == {'a': [1, True, None]}, 'loads bytes object input'
assert json.loads(b'123') == 123, 'loads bytes integer'
assert json.loads(b'1.5') == 1.5, 'loads bytes float'
assert json.loads(b'"hello"') == 'hello', 'loads bytes string'
assert json.loads(b'[1, 2, 3]') == [1, 2, 3], 'loads bytes array'
assert json.loads(b'true') is True, 'loads bytes true'
assert json.loads(b'false') is False, 'loads bytes false'
assert json.loads(b'null') is None, 'loads bytes null'
assert json.loads('"\\u2603"') == '☃', 'loads unicode escape'
assert json.loads('{"a": 1, "a": 2}') == {'a': 2}, 'loads duplicate object keys with last value winning'

# === loads big integers ===
big = 1234567890123456789012345678901234567890
assert json.loads(str(big)) == big, 'loads big integer as Python int'

# === dumps basics ===
assert json.dumps(None) == 'null', 'dumps None'
assert json.dumps(True) == 'true', 'dumps True'
assert json.dumps(False) == 'false', 'dumps False'
assert json.dumps(123) == '123', 'dumps int'
assert json.dumps(1.0) == '1.0', 'dumps whole float keeps decimal'
assert json.dumps(1.5) == '1.5', 'dumps float'
assert json.dumps('hello') == '"hello"', 'dumps string'
assert json.dumps([1, 2, 3]) == '[1, 2, 3]', 'dumps list'
assert json.dumps({'a': 1}) == '{"a": 1}', 'dumps dict'
assert json.dumps((1, 2, 3)) == '[1, 2, 3]', 'dumps tuple as array'

# === dumps formatting ===
assert json.dumps({'b': 1, 'a': 2}, sort_keys=True) == ('{"a": 2, "b": 1}'), 'sort_keys sorts string keys'
assert json.dumps({'a': [1, 2]}, indent=2) == ('{\n  "a": [\n    1,\n    2\n  ]\n}'), 'indent=2 pretty prints'
assert json.dumps({'a': [1, 2]}, indent='--') == ('{\n--"a": [\n----1,\n----2\n--]\n}'), (
    'string indent repeats per depth'
)
assert json.dumps({'a': 1}, separators=(',', ':')) == '{"a":1}', 'custom separators applied'

# === dumps unicode and floats ===
assert json.dumps('☃') == '"\\u2603"', 'ensure_ascii defaults to true'
assert json.dumps('☃', ensure_ascii=False) == ('"☃"'), 'ensure_ascii=False keeps unicode characters'
assert json.dumps(float('nan')) == 'NaN', 'allow_nan defaults to true'
assert json.dumps(float('inf')) == ('Infinity'), 'positive infinity serializes when allow_nan is true'
assert json.dumps(float('-inf')) == ('-Infinity'), 'negative infinity serializes when allow_nan is true'

# === dumps key coercion ===
assert (
    json.dumps({True: 1, False: 2, None: 3, 4: 5, 1.5: 6}) == '{"true": 1, "false": 2, "null": 3, "4": 5, "1.5": 6}'
), 'non-string JSON keys are coerced to strings like CPython'

# === dumps skipkeys ===
assert json.dumps({(1, 2): 3}, skipkeys=True) == '{}', 'skipkeys drops unsupported keys'

# === empty containers ===
assert json.dumps([]) == '[]', 'dumps empty list'
assert json.dumps({}) == '{}', 'dumps empty dict'
assert json.loads('[]') == [], 'loads empty list'
assert json.loads('{}') == {}, 'loads empty dict'

# === roundtrip ===
data = {'a': [1, 2.5, True, None, 'x', {'b': [3]}]}
assert json.loads(json.dumps(data)) == data, 'loads(dumps(x)) roundtrips nested JSON values'

# === JSONDecodeError subclassing ===
try:
    json.loads('{]')
    assert False, 'invalid JSON should raise JSONDecodeError'
except json.JSONDecodeError as exc:
    assert str(exc) == 'Expecting property name enclosed in double quotes: line 1 column 2 (char 1)', (
        'JSONDecodeError message matches CPython format'
    )

caught_value_error = False
try:
    json.loads('{]')
except ValueError:
    caught_value_error = True
assert caught_value_error, 'JSONDecodeError is catchable as ValueError'

# === dumps error handling ===
try:
    json.dumps(float('nan'), allow_nan=False)
    assert False, 'allow_nan=False should reject NaN'
except ValueError as exc:
    assert str(exc) == 'Out of range float values are not JSON compliant: nan', 'allow_nan=False error message'

try:
    json.dumps({(1, 2): 3})
    assert False, 'unsupported dict key type should raise TypeError'
except TypeError as exc:
    assert str(exc) == 'keys must be str, int, float, bool or None, not tuple', 'invalid key type error message'

try:
    json.dumps({1})
    assert False, 'set should not be JSON serializable'
except TypeError as exc:
    assert str(exc) == 'Object of type set is not JSON serializable', 'set serialization error message'

try:
    json.loads(1)
    assert False, 'loads(int) should raise TypeError'
except TypeError as exc:
    assert str(exc) == 'the JSON object must be str, bytes or bytearray, not int', 'loads type error message'

# === circular reference detection ===
circular = []
circular.append(circular)
try:
    json.dumps(circular)
    assert False, 'circular reference should raise ValueError'
except ValueError as exc:
    assert str(exc) == 'Circular reference detected', 'circular reference error message'
