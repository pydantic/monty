import sys
import tomllib

# === TOMLDecodeError messages and positions ===
# Each case is (source, str(exc)); the suffix locates the error the way
# CPython's TOMLDecodeError.__init__ does from `doc` and `pos`.
decode_errors = [
    # statements
    ('?', 'Invalid statement (at line 1, column 1)'),
    ('a = 1 b = 2', 'Expected newline or end of document after a statement (at line 1, column 7)'),
    ('a = 1\nb = 2 c', 'Expected newline or end of document after a statement (at line 2, column 7)'),
    ('a', "Expected '=' after a key in a key/value pair (at end of document)"),
    ('a b = 1', "Expected '=' after a key in a key/value pair (at line 1, column 3)"),
    ('a =', 'Invalid value (at end of document)'),
    ('a = ', 'Invalid value (at end of document)'),
    ('a = @', 'Invalid value (at line 1, column 5)'),
    ('a = tru', 'Invalid value (at line 1, column 5)'),
    ('a = True', 'Invalid value (at line 1, column 5)'),
    ('a. = 1', 'Invalid initial character for a key part (at line 1, column 4)'),
    ('a.é = 1', 'Invalid initial character for a key part (at line 1, column 3)'),
    ('é = 1', 'Invalid statement (at line 1, column 1)'),
    ('[a', "Expected ']' at the end of a table declaration (at end of document)"),
    ('[a b]', "Expected ']' at the end of a table declaration (at line 1, column 4)"),
    ('[[a]', "Expected ']]' at the end of an array declaration (at line 1, column 4)"),
    ('[[a', "Expected ']]' at the end of an array declaration (at end of document)"),
    ('[]', 'Invalid initial character for a key part (at line 1, column 2)'),
    ('a = 1 # comment \x01', "Found invalid character '\\x01' (at line 1, column 17)"),
    ('# \x7f', "Found invalid character '\\x7f' (at line 1, column 3)"),
    # numbers that stop early
    ('a = 1_', 'Expected newline or end of document after a statement (at line 1, column 6)'),
    ('a = 1__0', 'Expected newline or end of document after a statement (at line 1, column 6)'),
    ('a = 01', 'Expected newline or end of document after a statement (at line 1, column 6)'),
    ('a = 0x', 'Expected newline or end of document after a statement (at line 1, column 6)'),
    ('a = 0xg', 'Expected newline or end of document after a statement (at line 1, column 6)'),
    ('a = -0x1', 'Expected newline or end of document after a statement (at line 1, column 7)'),
    ('a = 1.', 'Expected newline or end of document after a statement (at line 1, column 6)'),
    ('a = .5', 'Invalid value (at line 1, column 5)'),
    ('a = 1e', 'Expected newline or end of document after a statement (at line 1, column 6)'),
    ('a = 1.2.3', 'Expected newline or end of document after a statement (at line 1, column 8)'),
    ('a = infinity', 'Expected newline or end of document after a statement (at line 1, column 8)'),
    ('a = +', 'Invalid value (at line 1, column 5)'),
    # dates and times
    ('a = 0000-01-01', 'Invalid date or datetime (at line 1, column 5)'),
    ('a = 1979-02-30', 'Invalid date or datetime (at line 1, column 5)'),
    ('a = 1979-05-32', 'Expected newline or end of document after a statement (at line 1, column 9)'),
    ('a = 1979-13-01', 'Expected newline or end of document after a statement (at line 1, column 9)'),
    ('a = 1979-05-27T', 'Expected newline or end of document after a statement (at line 1, column 15)'),
    ('a = 1979-05-27T07:32', 'Expected newline or end of document after a statement (at line 1, column 15)'),
    ('a = 1979-05-27T24:00:00', 'Expected newline or end of document after a statement (at line 1, column 15)'),
    ('a = 1979-05-27T07:32:00+24:00', 'Expected newline or end of document after a statement (at line 1, column 24)'),
    ('a = 1979-05-27T07:32:00+01:60', 'Expected newline or end of document after a statement (at line 1, column 24)'),
    ('a = 1979-05-27T07:32:00+0100', 'Expected newline or end of document after a statement (at line 1, column 24)'),
    ('a = 07:60:00', 'Expected newline or end of document after a statement (at line 1, column 6)'),
    ('a = 07:32:00.', 'Expected newline or end of document after a statement (at line 1, column 13)'),
    # strings
    ('a = "x', 'Unterminated string (at end of document)'),
    ('a = "x\ny"', "Illegal character '\\n' (at line 1, column 7)"),
    ('a = "x\x01"', "Illegal character '\\x01' (at line 1, column 7)"),
    ('a = "é\x01"', "Illegal character '\\x01' (at line 1, column 7)"),
    ('a = "\x7f"', "Illegal character '\\x7f' (at line 1, column 6)"),
    ('a = "\\x41"', "Unescaped '\\' in a string (at line 1, column 8)"),
    ('a = "\\', "Unescaped '\\' in a string (at end of document)"),
    ('a = "\\é', "Unescaped '\\' in a string (at end of document)"),
    ('a = "\\é rest"', "Unescaped '\\' in a string (at line 1, column 8)"),
    ('a = "\\q"', "Unescaped '\\' in a string (at line 1, column 8)"),
    ('a = "\\ "', "Unescaped '\\' in a string (at line 1, column 8)"),
    ('a = "\\u12"', 'Invalid hex value (at line 1, column 8)'),
    ('a = "\\u12G4"', 'Invalid hex value (at line 1, column 8)'),
    ('a = "\\U1234567"', 'Invalid hex value (at line 1, column 8)'),
    ('a = "\\uD800"', 'Escaped character is not a Unicode scalar value (at line 1, column 12)'),
    ('a = "\\U00110000"', 'Escaped character is not a Unicode scalar value (at line 1, column 16)'),
    ("a = 'x", 'Expected "\'" (at end of document)'),
    ("a = 'x\ny'", "Found invalid character '\\n' (at line 1, column 7)"),
    ("a = 'x\x01'", "Found invalid character '\\x01' (at line 1, column 7)"),
    ('a = """x', 'Unterminated string (at end of document)'),
    ('a = """x\x01"""', "Illegal character '\\x01' (at line 1, column 9)"),
    ('a = """\\ x\n"""', "Unescaped '\\' in a string (at line 1, column 10)"),
    ("a = '''x", "Expected \"'''\" (at end of document)"),
    ("a = '''\n\x7f'''", "Found invalid character '\\x7f' (at line 2, column 1)"),
    ("a = '''x\r'''", "Found invalid character '\\r' (at line 1, column 9)"),
    # arrays and inline tables
    ('a = [1 2]', 'Unclosed array (at line 1, column 8)'),
    ('a = [1,,]', 'Invalid value (at line 1, column 8)'),
    ('a = [1', 'Unclosed array (at end of document)'),
    ('a = [', 'Invalid value (at end of document)'),
    ('a = {b = 1', 'Unclosed inline table (at end of document)'),
    ('a = {b = 1 c = 2}', 'Unclosed inline table (at line 1, column 12)'),
    ('a = {b = 1,}', 'Invalid initial character for a key part (at line 1, column 12)'),
    ('a = {b = 1,\n}', 'Invalid initial character for a key part (at line 1, column 12)'),
    ('a = {\nb = 1}', 'Invalid initial character for a key part (at line 1, column 6)'),
    ('a = {b = 1, b = 2}', "Duplicate inline table key 'b' (at line 1, column 18)"),
    ('a = {\'b c\' = 1, "b c" = 2}', "Duplicate inline table key 'b c' (at line 1, column 26)"),
    ('a = {b = {}, b.c = 1}', "Cannot mutate immutable namespace ('b', 'c') (at line 1, column 21)"),
    ('a = {b = [], b.c = 1}', "Cannot mutate immutable namespace ('b', 'c') (at line 1, column 21)"),
    ('a = {b = 1, b.c = 2}', 'Cannot overwrite a value (at line 1, column 20)'),
    # redefinition
    ('a = 1\na = 2', 'Cannot overwrite a value (at end of document)'),
    ('a = 1\na = 2\n', 'Cannot overwrite a value (at line 2, column 6)'),
    ('a = 1\n[a.b]', 'Cannot overwrite a value (at line 2, column 5)'),
    ('a = 1\n[[a]]', 'Cannot overwrite a value (at line 2, column 4)'),
    ('a = 1\na.b = 2', 'Cannot overwrite a value (at end of document)'),
    ('[a]\n[a]', "Cannot declare ('a',) twice (at line 2, column 3)"),
    ('[a.b]\n[a.b]', "Cannot declare ('a', 'b') twice (at line 2, column 5)"),
    ('a.b = 1\n[a]', "Cannot declare ('a',) twice (at line 2, column 3)"),
    ('[a]\nb.c = 1\n[a.b]', "Cannot declare ('a', 'b') twice (at line 3, column 5)"),
    ('a = {}\n[a.b]', "Cannot declare ('a', 'b') twice (at line 2, column 5)"),
    ('a = {}\n[a]', "Cannot declare ('a',) twice (at line 2, column 3)"),
    ('[a]\nb = 1\n[a.b]', 'Cannot overwrite a value (at line 3, column 5)'),
    ('[a.b]\n[a]\nb.c = 1', "Cannot redefine namespace ('a', 'b') (at end of document)"),
    ('[a.b]\n[a]\nb.c = 1\n', "Cannot redefine namespace ('a', 'b') (at line 3, column 8)"),
    ('a = []\n[[a]]', "Cannot mutate immutable namespace ('a',) (at line 2, column 4)"),
    ('a = [{}]\n[a.b]', "Cannot declare ('a', 'b') twice (at line 2, column 5)"),
    ('a = {b = 1}\na.c = 2', "Cannot mutate immutable namespace ('a',) (at end of document)"),
    ('[[a]]\n[a]', "Cannot declare ('a',) twice (at line 2, column 3)"),
    ('[a]\n[[a]]', 'Cannot overwrite a value (at line 2, column 4)'),
    ('x = 1\n"x" = 2', 'Cannot overwrite a value (at end of document)'),
    # positions count characters, not bytes
    ('é = 1\n☃ = 2', 'Invalid statement (at line 1, column 1)'),
    ('"☃☃" = 1 x', 'Expected newline or end of document after a statement (at line 1, column 10)'),
    ('a = "☃☃☃\x01"', "Illegal character '\\x01' (at line 1, column 9)"),
    ('a = "☃\r\n"', "Illegal character '\\n' (at line 1, column 7)"),
]
for source, expected in decode_errors:
    try:
        tomllib.loads(source)
        assert False, f'expected {source!r} to fail'
    except tomllib.TOMLDecodeError as exc:
        assert str(exc) == expected, (source, str(exc))

# === TOMLDecodeError is a ValueError ===
try:
    tomllib.loads('?')
    assert False, 'expected TOMLDecodeError'
except ValueError as exc:
    assert isinstance(exc, tomllib.TOMLDecodeError)
    assert type(exc).__name__ == 'TOMLDecodeError'
    assert str(type(exc)) == "<class 'tomllib.TOMLDecodeError'>"
    assert exc.args == ('Invalid statement (at line 1, column 1)',)

# === non-str input ===
for bad in [b'a = 1', 1, None, ['a = 1']]:
    try:
        tomllib.loads(bad)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == f"Expected str object, not '{type(bad).__name__}'"

# === signature ===
try:
    tomllib.loads()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "loads() missing 1 required positional argument: 's'"
try:
    tomllib.loads('a = 1', 'extra')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'loads() takes 1 positional argument but 2 were given'
# `parse_float` is not implemented; CPython accepts it
if sys.platform == 'monty':
    try:
        tomllib.loads('a = 1', parse_float=float)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == "loads() got an unexpected keyword argument 'parse_float'"

# === integer digit limit (ValueError from int(), not TOMLDecodeError) ===
try:
    tomllib.loads('x = ' + '1' * 4301)
    assert False, 'expected ValueError'
except tomllib.TOMLDecodeError:
    assert False, 'digit limit is a plain ValueError'
except ValueError as exc:
    assert str(exc) == (
        'Exceeds the limit (4300 digits) for integer string conversion: value has 4301 digits; '
        'use sys.set_int_max_str_digits() to increase the limit'
    )
assert tomllib.loads('x = 0x' + 'f' * 5000)['x'] == int('f' * 5000, 16)

# === deeply dotted keys ===
try:
    tomllib.loads('.'.join(['a'] * 1001) + ' = 1')
    assert False, 'expected RecursionError'
except RecursionError as exc:
    assert str(exc) == 'TOML key has more than the allowed 1000 parts'
nested = tomllib.loads('.'.join(['a'] * 1000) + ' = 1')
for _ in range(999):
    nested = nested['a']
assert nested == {'a': 1}

# === nested containers within the shared depth ===
depth = 100
nested_list = tomllib.loads('x = ' + '[' * depth + '1' + ']' * depth)['x']
nested_dict = tomllib.loads('x = ' + '{a = ' * depth + '1' + '}' * depth)['x']
for _ in range(depth):
    assert type(nested_list) is list and len(nested_list) == 1
    assert type(nested_dict) is dict and list(nested_dict) == ['a']
    nested_list = nested_list[0]
    nested_dict = nested_dict['a']
assert nested_list == 1
assert nested_dict == 1
try:
    tomllib.loads('x = ' + '[' * 10_000 + '1' + ']' * 10_000)
    assert False, 'expected RecursionError'
except RecursionError as exc:
    assert str(exc) == 'maximum recursion depth exceeded'
