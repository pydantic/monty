import json

# === dumps unicode and escaping ===
assert json.dumps('😀') == '"\\ud83d\\ude00"'
assert json.dumps('😀', ensure_ascii=False) == '"😀"'
assert json.dumps('A☃😀') == '"A\\u2603\\ud83d\\ude00"'
assert json.dumps('A☃😀', ensure_ascii=False) == '"A☃😀"'
assert json.dumps('\b\f\n\r\t') == '"\\b\\f\\n\\r\\t"'
assert json.dumps({'☃': '😀'}) == '{"\\u2603": "\\ud83d\\ude00"}'
assert json.dumps({'☃': '😀'}, ensure_ascii=False) == '{"☃": "😀"}'

# === dumps indentation and separators ===
assert json.dumps({'a': [1, 2]}, indent=0) == '{\n"a": [\n1,\n2\n]\n}'
assert json.dumps({'a': [1, 2]}, indent=-1) == '{\n"a": [\n1,\n2\n]\n}'
assert json.dumps({'a': [1, 2]}, indent=True) == '{\n "a": [\n  1,\n  2\n ]\n}'
assert json.dumps({'a': 1}, separators=None) == '{"a": 1}'

# === dumps exact numeric literals ===
big = 1234567890123456789012345678901234567890
assert json.dumps(big) == '1234567890123456789012345678901234567890'
assert json.dumps({big: 1}) == '{"1234567890123456789012345678901234567890": 1}'
assert json.dumps(1e20) == '1e+20'
assert json.dumps(1e-6) == '1e-06'
assert json.dumps(-0.0) == '-0.0'
assert json.dumps(9999999999999998.0) == '9999999999999998.0'
assert json.dumps(1.0000000000000002e16) == '1.0000000000000002e+16'
assert json.dumps(0.0001) == '0.0001'
assert json.dumps(9.999999999999999e-05) == '9.999999999999999e-05'
assert json.dumps(5e-324) == '5e-324'
assert json.dumps(1e300) == '1e+300'

# === loads unicode literals ===
assert json.loads('"☃😀"') == '☃😀'
assert json.loads('{"☃": "😀"}') == {'☃': '😀'}
assert json.loads('"\\ud83d\\ude00"') == '😀'
assert json.loads('"☃😀"'.encode('utf-8')) == '☃😀'

# === loads numeric literals ===
assert json.loads(str(big)) == big
assert json.loads('1e20') == 1e20
assert json.loads('1e-6') == 1e-6
assert json.loads('-0.0') == -0.0

# === loads NaN and Infinity (CPython accepts these by default) ===
import math

nan_result = json.loads('NaN')
assert math.isnan(nan_result)
assert json.loads('Infinity') == float('inf')
assert json.loads('-Infinity') == float('-inf')
assert json.loads('[NaN, Infinity, -Infinity]')[1] == float('inf')
