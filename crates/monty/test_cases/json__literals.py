import json

# === dumps unicode and escaping ===
assert json.dumps('😀') == '"\\ud83d\\ude00"', 'ensure_ascii escapes supplementary-plane characters as surrogate pairs'
assert json.dumps('😀', ensure_ascii=False) == '"😀"', 'ensure_ascii=False keeps supplementary-plane characters'
assert json.dumps('A☃😀') == '"A\\u2603\\ud83d\\ude00"', (
    'mixed non-ASCII string escapes all non-ASCII code points by default'
)
assert json.dumps('A☃😀', ensure_ascii=False) == '"A☃😀"', (
    'mixed non-ASCII string stays literal with ensure_ascii=False'
)
assert json.dumps('\b\f\n\r\t') == '"\\b\\f\\n\\r\\t"', 'control characters use the short JSON escapes'
assert json.dumps({'☃': '😀'}) == '{"\\u2603": "\\ud83d\\ude00"}', 'non-ASCII dict keys and values escape by default'
assert json.dumps({'☃': '😀'}, ensure_ascii=False) == '{"☃": "😀"}', (
    'non-ASCII dict keys and values stay literal with ensure_ascii=False'
)

# === dumps indentation and separators ===
assert json.dumps({'a': [1, 2]}, indent=0) == '{\n"a": [\n1,\n2\n]\n}', 'indent=0 uses newline-only pretty printing'
assert json.dumps({'a': [1, 2]}, indent=-1) == '{\n"a": [\n1,\n2\n]\n}', (
    'negative indent matches indent=0 newline-only formatting'
)
assert json.dumps({'a': [1, 2]}, indent=True) == '{\n "a": [\n  1,\n  2\n ]\n}', 'indent=True behaves like indent=1'
assert json.dumps({'a': 1}, separators=None) == '{"a": 1}', 'separators=None keeps the default separators'

# === dumps exact numeric literals ===
big = 1234567890123456789012345678901234567890
assert json.dumps(big) == '1234567890123456789012345678901234567890', 'big integers dump without scientific notation'
assert json.dumps({big: 1}) == '{"1234567890123456789012345678901234567890": 1}', (
    'big integer dict keys are coerced to decimal strings'
)
assert json.dumps(1e20) == '1e+20', 'large finite floats keep exponent notation'
assert json.dumps(1e-6) == '1e-06', 'small finite floats keep exponent notation'
assert json.dumps(-0.0) == '-0.0', 'negative zero preserves its sign when dumped'
assert json.dumps(9999999999999998.0) == '9999999999999998.0', (
    'float just below 1e16 stays in fixed notation despite log10 rounding'
)
assert json.dumps(1.0000000000000002e16) == '1.0000000000000002e+16', 'float just above 1e16 uses exponent notation'
assert json.dumps(0.0001) == '0.0001', 'float at 1e-4 boundary stays in fixed notation'
assert json.dumps(9.999999999999999e-05) == '9.999999999999999e-05', 'float just below 1e-4 uses exponent notation'
assert json.dumps(5e-324) == '5e-324', 'smallest subnormal float uses exponent notation'
assert json.dumps(1e300) == '1e+300', 'very large float uses exponent notation'

# === loads unicode literals ===
assert json.loads('"☃😀"') == '☃😀', 'loads raw non-ASCII JSON strings'
assert json.loads('{"☃": "😀"}') == {'☃': '😀'}, 'loads raw non-ASCII object keys and values'
assert json.loads('"\\ud83d\\ude00"') == '😀', 'loads surrogate-pair escapes into a supplementary-plane character'
assert json.loads('"☃😀"'.encode('utf-8')) == '☃😀', 'loads UTF-8 bytes containing raw non-ASCII characters'

# === loads numeric literals ===
assert json.loads(str(big)) == big, 'loads large integers without losing precision'
assert json.loads('1e20') == 1e20, 'loads large exponent floats'
assert json.loads('1e-6') == 1e-6, 'loads small exponent floats'
assert json.loads('-0.0') == -0.0, 'loads negative zero as a float'

# === loads NaN and Infinity (CPython accepts these by default) ===
import math

nan_result = json.loads('NaN')
assert math.isnan(nan_result), 'loads NaN as float nan'
assert json.loads('Infinity') == float('inf'), 'loads Infinity as float inf'
assert json.loads('-Infinity') == float('-inf'), 'loads -Infinity as float -inf'
assert json.loads('[NaN, Infinity, -Infinity]')[1] == float('inf'), 'loads NaN/Infinity in arrays'
