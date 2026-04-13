# === repr of None, True, False, Ellipsis ===
assert repr(None) == 'None', 'repr(None)'
assert repr(True) == 'True', 'repr(True)'
assert repr(False) == 'False', 'repr(False)'
assert repr(...) == 'Ellipsis', 'repr(Ellipsis)'

# === repr of ints (Value::Int) ===
assert repr(0) == '0', 'repr(0)'
assert repr(1) == '1', 'repr(1)'
assert repr(-1) == '-1', 'repr(-1)'
assert repr(42) == '42', 'repr(42)'
assert repr(-999) == '-999', 'repr(-999)'
assert repr(9223372036854775807) == '9223372036854775807', 'repr(max i64)'
assert repr(-9223372036854775808) == '-9223372036854775808', 'repr(min i64)'

# === repr of big ints (Value::InternLongInt / HeapData::LongInt) ===
assert repr(9223372036854775808) == '9223372036854775808', 'repr(i64 + 1)'
assert repr(-9223372036854775809) == '-9223372036854775809', 'repr(i64 - 1)'
assert repr(10**20) == '100000000000000000000', 'repr(10**20)'
assert repr(-(10**20)) == '-100000000000000000000', 'repr(-(10**20))'

# === repr of floats (Value::Float) ===
assert repr(0.0) == '0.0', 'repr(0.0)'
assert repr(-0.0) == '-0.0', 'repr(-0.0)'
assert repr(1.0) == '1.0', 'repr(1.0)'
assert repr(2.0) == '2.0', 'repr(2.0) integer-like float gets .0'
assert repr(0.5) == '0.5', 'repr(0.5)'
assert repr(2.5) == '2.5', 'repr(2.5)'
assert repr(100.0) == '100.0', 'repr(100.0)'
assert repr(-3.14) == '-3.14', 'repr(-3.14)'
assert repr(0.1) == '0.1', 'repr(0.1)'

# === repr of strings (Value::InternString / HeapData::Str) ===
assert repr('') == "''", 'repr empty string'
assert repr('hello') == "'hello'", 'repr simple string'
assert repr("it's") == '"it\'s"', 'repr string with single quote uses double quotes'
assert repr('say "hi"') == '\'say "hi"\'', 'repr string with double quotes uses single quotes'
assert repr('it\'s "complex"') == "'it\\'s \"complex\"'", 'repr string with both quotes'
assert repr('a\nb') == "'a\\nb'", 'repr string with newline'
assert repr('a\tb') == "'a\\tb'", 'repr string with tab'
assert repr('a\\b') == "'a\\\\b'", 'repr string with backslash'

# === repr of bytes (Value::InternBytes) ===
assert repr(b'') == "b''", 'repr empty bytes'
assert repr(b'hello') == "b'hello'", 'repr simple bytes'
assert repr(b'\x00') == "b'\\x00'", 'repr bytes with null'
assert repr(b'\xff') == "b'\\xff'", 'repr bytes with 0xff'
assert repr(b"it's") == 'b"it\'s"', 'repr bytes with single quote'

# === repr of built-in functions (Value::Builtin) ===
assert repr(len) == '<built-in function len>', 'repr(len)'
assert repr(print) == '<built-in function print>', 'repr(print)'
assert repr(repr) == '<built-in function repr>', 'repr(repr)'
assert repr(abs) == '<built-in function abs>', 'repr(abs)'
assert repr(min) == '<built-in function min>', 'repr(min)'
assert repr(max) == '<built-in function max>', 'repr(max)'
assert repr(sorted) == '<built-in function sorted>', 'repr(sorted)'
assert repr(isinstance) == '<built-in function isinstance>', 'repr(isinstance)'
assert repr(hash) == '<built-in function hash>', 'repr(hash)'
assert repr(id) == '<built-in function id>', 'repr(id)'
assert repr(bin) == '<built-in function bin>', 'repr(bin)'
assert repr(hex) == '<built-in function hex>', 'repr(hex)'
assert repr(oct) == '<built-in function oct>', 'repr(oct)'
assert repr(ord) == '<built-in function ord>', 'repr(ord)'
assert repr(chr) == '<built-in function chr>', 'repr(chr)'

# === repr of type objects (Value::Marker) ===
assert repr(int) == "<class 'int'>", 'repr(int)'
assert repr(str) == "<class 'str'>", 'repr(str)'
assert repr(float) == "<class 'float'>", 'repr(float)'
assert repr(list) == "<class 'list'>", 'repr(list)'
assert repr(dict) == "<class 'dict'>", 'repr(dict)'
assert repr(tuple) == "<class 'tuple'>", 'repr(tuple)'
assert repr(set) == "<class 'set'>", 'repr(set)'
assert repr(bool) == "<class 'bool'>", 'repr(bool)'
assert repr(range) == "<class 'range'>", 'repr(range)'
assert repr(bytes) == "<class 'bytes'>", 'repr(bytes)'
assert repr(frozenset) == "<class 'frozenset'>", 'repr(frozenset)'

# === repr of lists (HeapData::List via Value::Ref) ===
assert repr([]) == '[]', 'repr empty list'
assert repr([1]) == '[1]', 'repr single-item list'
assert repr([1, 2, 3]) == '[1, 2, 3]', 'repr list of ints'
assert repr(['a', 'b']) == "['a', 'b']", 'repr list of strings'
assert repr([True, False]) == '[True, False]', 'repr list of bools'
assert repr([None]) == '[None]', 'repr list of None'
assert repr([[1, 2], [3]]) == '[[1, 2], [3]]', 'repr nested lists'
assert repr([1, 'a', True, None]) == "[1, 'a', True, None]", 'repr mixed list'

# === repr of tuples (HeapData::Tuple via Value::Ref) ===
assert repr(()) == '()', 'repr empty tuple'
assert repr((1, 2)) == '(1, 2)', 'repr tuple of ints'
assert repr((1, 2, 3)) == '(1, 2, 3)', 'repr triple tuple'
assert repr(('a', 'b')) == "('a', 'b')", 'repr tuple of strings'

# === repr of dicts (HeapData::Dict via Value::Ref) ===
assert repr({}) == '{}', 'repr empty dict'
assert repr({1: 'a'}) == "{1: 'a'}", 'repr single-item dict'
assert repr({1: 'a', 'b': 2}) == "{1: 'a', 'b': 2}", 'repr mixed dict'
assert repr({'key': [1, 2]}) == "{'key': [1, 2]}", 'repr dict with list value'
assert repr({'nested': {'a': 1}}) == "{'nested': {'a': 1}}", 'repr nested dict'

# === repr of sets (HeapData::Set via Value::Ref) ===
assert repr(set()) == 'set()', 'repr empty set'
assert repr({1}) == '{1}', 'repr single-item set'

# === repr of frozensets (HeapData::FrozenSet via Value::Ref) ===
assert repr(frozenset()) == 'frozenset()', 'repr empty frozenset'

# === repr of ranges (HeapData::Range via Value::Ref) ===
assert repr(range(10)) == 'range(0, 10)', 'repr range(10)'
assert repr(range(1, 10)) == 'range(1, 10)', 'repr range(1, 10)'
assert repr(range(0, 10, 2)) == 'range(0, 10, 2)', 'repr range with step'
assert repr(range(0)) == 'range(0, 0)', 'repr range(0)'
assert repr(range(-5, 5)) == 'range(-5, 5)', 'repr range with negative start'

# === repr of slices (HeapData::Slice via Value::Ref) ===
assert repr(slice(5)) == 'slice(None, 5, None)', 'repr slice(5)'
assert repr(slice(1, 5)) == 'slice(1, 5, None)', 'repr slice(1, 5)'
assert repr(slice(1, 10, 2)) == 'slice(1, 10, 2)', 'repr slice with step'
assert repr(slice(None)) == 'slice(None, None, None)', 'repr slice(None)'
assert repr(slice(None, None, -1)) == 'slice(None, None, -1)', 'repr slice reverse'

# === repr of nested/mixed containers ===
assert repr([1, 'hello', True, None, 2.5]) == "[1, 'hello', True, None, 2.5]", 'repr mixed list'
assert repr((1, [2, 3], {'a': 'b'})) == "(1, [2, 3], {'a': 'b'})", 'repr tuple with nested containers'
assert repr({'k': (1, 2)}) == "{'k': (1, 2)}", 'repr dict with tuple value'
assert repr([range(3)]) == '[range(0, 3)]', 'repr list containing range'

# === repr preserves insertion order in dicts ===
d = {}
d['z'] = 1
d['a'] = 2
d['m'] = 3
assert repr(d) == "{'z': 1, 'a': 2, 'm': 3}", 'repr dict preserves insertion order'

# === repr vs str difference ===
assert repr(42) == str(42), 'repr and str match for int'
assert repr('hello') != str('hello'), 'repr and str differ for string'
assert repr('hello') == "'hello'", 'repr adds quotes to string'
assert str('hello') == 'hello', 'str does not add quotes to string'
