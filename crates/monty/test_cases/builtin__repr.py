# === repr of None, True, False, Ellipsis ===
assert repr(None) == 'None'
assert repr(True) == 'True'
assert repr(False) == 'False'
assert repr(...) == 'Ellipsis'

# === repr of ints (Value::Int) ===
assert repr(0) == '0'
assert repr(1) == '1'
assert repr(-1) == '-1'
assert repr(42) == '42'
assert repr(-999) == '-999'
assert repr(9223372036854775807) == '9223372036854775807'
assert repr(-9223372036854775808) == '-9223372036854775808'

# === repr of big ints (Value::InternLongInt / HeapData::LongInt) ===
assert repr(9223372036854775808) == '9223372036854775808'
assert repr(-9223372036854775809) == '-9223372036854775809'
assert repr(10**20) == '100000000000000000000'
assert repr(-(10**20)) == '-100000000000000000000'

# === repr of floats (Value::Float) ===
assert repr(0.0) == '0.0'
assert repr(-0.0) == '-0.0'
assert repr(1.0) == '1.0'
assert repr(2.0) == '2.0'
assert repr(0.5) == '0.5'
assert repr(2.5) == '2.5'
assert repr(100.0) == '100.0'
assert repr(-3.14) == '-3.14'
assert repr(0.1) == '0.1'

# === repr of strings (Value::InternString / HeapData::Str) ===
assert repr('') == "''"
assert repr('hello') == "'hello'"
assert repr("it's") == '"it\'s"'
assert repr('say "hi"') == '\'say "hi"\''
assert repr('it\'s "complex"') == "'it\\'s \"complex\"'"
assert repr('a\nb') == "'a\\nb'"
assert repr('a\tb') == "'a\\tb'"
assert repr('a\\b') == "'a\\\\b'"

# === repr of bytes (Value::InternBytes) ===
assert repr(b'') == "b''"
assert repr(b'hello') == "b'hello'"
assert repr(b'\x00') == "b'\\x00'"
assert repr(b'\xff') == "b'\\xff'"
assert repr(b"it's") == 'b"it\'s"'

# === repr of built-in functions (Value::Builtin) ===
assert repr(len) == '<built-in function len>'
assert repr(print) == '<built-in function print>'
assert repr(repr) == '<built-in function repr>'
assert repr(abs) == '<built-in function abs>'
assert repr(min) == '<built-in function min>'
assert repr(max) == '<built-in function max>'
assert repr(sorted) == '<built-in function sorted>'
assert repr(isinstance) == '<built-in function isinstance>'
assert repr(hash) == '<built-in function hash>'
assert repr(id) == '<built-in function id>'
assert repr(bin) == '<built-in function bin>'
assert repr(hex) == '<built-in function hex>'
assert repr(oct) == '<built-in function oct>'
assert repr(ord) == '<built-in function ord>'
assert repr(chr) == '<built-in function chr>'

# === repr of type objects (Value::Marker) ===
assert repr(int) == "<class 'int'>"
assert repr(str) == "<class 'str'>"
assert repr(float) == "<class 'float'>"
assert repr(list) == "<class 'list'>"
assert repr(dict) == "<class 'dict'>"
assert repr(tuple) == "<class 'tuple'>"
assert repr(set) == "<class 'set'>"
assert repr(bool) == "<class 'bool'>"
assert repr(range) == "<class 'range'>"
assert repr(bytes) == "<class 'bytes'>"
assert repr(frozenset) == "<class 'frozenset'>"

# === repr of lists (HeapData::List via Value::Ref) ===
assert repr([]) == '[]'
assert repr([1]) == '[1]'
assert repr([1, 2, 3]) == '[1, 2, 3]'
assert repr(['a', 'b']) == "['a', 'b']"
assert repr([True, False]) == '[True, False]'
assert repr([None]) == '[None]'
assert repr([[1, 2], [3]]) == '[[1, 2], [3]]'
assert repr([1, 'a', True, None]) == "[1, 'a', True, None]"

# === repr of tuples (HeapData::Tuple via Value::Ref) ===
assert repr(()) == '()'
assert repr((1, 2)) == '(1, 2)'
assert repr((1, 2, 3)) == '(1, 2, 3)'
assert repr(('a', 'b')) == "('a', 'b')"

# === repr of dicts (HeapData::Dict via Value::Ref) ===
assert repr({}) == '{}'
assert repr({1: 'a'}) == "{1: 'a'}"
assert repr({1: 'a', 'b': 2}) == "{1: 'a', 'b': 2}"
assert repr({'key': [1, 2]}) == "{'key': [1, 2]}"
assert repr({'nested': {'a': 1}}) == "{'nested': {'a': 1}}"

# === repr of sets (HeapData::Set via Value::Ref) ===
assert repr(set()) == 'set()'
assert repr({1}) == '{1}'

# === repr of frozensets (HeapData::FrozenSet via Value::Ref) ===
assert repr(frozenset()) == 'frozenset()'

# === repr of ranges (HeapData::Range via Value::Ref) ===
assert repr(range(10)) == 'range(0, 10)'
assert repr(range(1, 10)) == 'range(1, 10)'
assert repr(range(0, 10, 2)) == 'range(0, 10, 2)'
assert repr(range(0)) == 'range(0, 0)'
assert repr(range(-5, 5)) == 'range(-5, 5)'

# === repr of slices (HeapData::Slice via Value::Ref) ===
assert repr(slice(5)) == 'slice(None, 5, None)'
assert repr(slice(1, 5)) == 'slice(1, 5, None)'
assert repr(slice(1, 10, 2)) == 'slice(1, 10, 2)'
assert repr(slice(None)) == 'slice(None, None, None)'
assert repr(slice(None, None, -1)) == 'slice(None, None, -1)'

# === repr of nested/mixed containers ===
assert repr([1, 'hello', True, None, 2.5]) == "[1, 'hello', True, None, 2.5]"
assert repr((1, [2, 3], {'a': 'b'})) == "(1, [2, 3], {'a': 'b'})"
assert repr({'k': (1, 2)}) == "{'k': (1, 2)}"
assert repr([range(3)]) == '[range(0, 3)]'

# === repr preserves insertion order in dicts ===
d = {}
d['z'] = 1
d['a'] = 2
d['m'] = 3
assert repr(d) == "{'z': 1, 'a': 2, 'm': 3}"

# === repr escapes control characters numerically ===
assert repr('\x00') == "'\\x00'"
assert repr('\x01') == "'\\x01'"
assert repr('\x1b') == "'\\x1b'"
assert repr('\x7f') == "'\\x7f'"
assert repr('\x80') == "'\\x80'"
assert repr('a\x00b') == "'a\\x00b'"
assert repr('\x0b') == "'\\x0b'"
assert repr('\x0c') == "'\\x0c'"
# the three short escapes are still preferred
assert repr('\t\n\r') == "'\\t\\n\\r'"
# printable characters (including unicode) are left as-is
assert repr('café 日本') == "'café 日本'"
# non-printable unicode beyond the control range is escaped too (Cf/Zs/Co/...)
assert repr('\xa0') == "'\\xa0'"
assert repr(' ') == "'\\u2028'"
assert repr('\xad') == "'\\xad'"
assert repr('\U000f4240') == "'\\U000f4240'"
assert repr('　') == "'\\u3000'"

# === repr vs str difference ===
assert repr(42) == str(42)
assert repr('hello') != str('hello')
assert repr('hello') == "'hello'"
assert str('hello') == 'hello'
