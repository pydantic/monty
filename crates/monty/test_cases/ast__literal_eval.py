import ast
from ast import literal_eval

assert type(id(literal_eval)) is int
assert repr(literal_eval).split(' at ')[0] == '<function literal_eval'

assert ast.literal_eval(node_or_string=' \t[True, False, None, ...]') == [True, False, None, ...]
assert literal_eval(r"""['one' "two", b'\x00' b'abc', '\u00e9']""") == ['onetwo', b'\x00abc', 'é']
assert literal_eval(r"'\0'") == '\0'
assert literal_eval("'\ufeff'") == '\ufeff'
assert literal_eval("[(+3, -2.5), {True: 1, 1: 2}, {'a', 'b', 'a'}]") == [(3, -2.5), {True: 2}, {'a', 'b'}]
assert literal_eval('1,') == (1,)
assert literal_eval('()') == ()
assert literal_eval('{}') == {}
assert literal_eval('-0x1_0000_0000_0000_0000') == -(2**64)
assert literal_eval('+12_345_678_901_234_567_890') == 12_345_678_901_234_567_890
assert literal_eval('[-2 + 3j, +4.5 - 2j, -3j, +1j]') == [-2 + 3j, 4.5 - 2j, -3j, 1j]
assert repr(literal_eval('-0j')) == '(-0-0j)'
assert literal_eval('\n# comment\n(1,\n2)\n') == (1, 2)
assert literal_eval("'''a\r\nb\rc\n\\r'''") == 'a\nb\nc\n\r'
assert literal_eval("b'''a\r\nb\rc'''") == b'a\nb\nc'
assert literal_eval('1\r\n') == 1
assert literal_eval('1\\\n ') == 1
assert literal_eval("'" + '(' * 4100 + "'") == '(' * 4100


def set():
    raise AssertionError('literal_eval called set')


assert type(literal_eval('(set)()')).__name__ == 'set'
assert type(literal_eval('ｓet()')).__name__ == 'set'

for source in (
    'set(())',
    'set(**{})',
    'name',
    '(lambda: 42)()',
    '(1).real',
    '(1,)[0]',
    '1 + 2',
    '1j + 2j',
    '1 + -2j',
    '1 * 2j',
    '++1',
    '~1',
    '-True',
    'f"{1}"',
    '[x for x in ()]',
    '[["heap-allocated string"], *()]',
    '{"heap-allocated key": [], **{}}',
    '{"heap-allocated key": unknown}',
    b'123',
    [1],
):
    try:
        literal_eval(source)
        assert False, 'expected ValueError'
    except ValueError:
        pass

for source in ('\0', "'a\0b'", '1 # \0'):
    try:
        literal_eval(source)
        assert False, 'expected SyntaxError'
    except SyntaxError as exc:
        assert str(exc) == 'source code string cannot contain null bytes'

try:
    literal_eval(' \t\ufeff1')
    assert False, 'expected SyntaxError'
except SyntaxError as exc:
    assert str(exc) == 'invalid non-printable character U+FEFF (<unknown>, line 1)'

for source in ('[1,', '1; 2', '1\n ', '1 # comment\\\n ', '1\\\n\n ', '[' * 201 + '0' + ']' * 201):
    try:
        literal_eval(source)
        assert False, 'expected SyntaxError'
    except SyntaxError:
        pass

assert literal_eval('9_' * 4299 + '9') == 10**4300 - 1
assert literal_eval('0' * 4301) == 0
assert literal_eval('0x' + 'f' * 4301) == 16**4301 - 1
over_limit = '9_' * 4300 + '9'
for source in (over_limit, 'f(' + over_limit + ')'):
    try:
        literal_eval(source)
        assert False, 'expected SyntaxError'
    except SyntaxError:
        pass

for source, message in (
    ('{1, []}', "cannot use 'list' as a set element (unhashable type: 'list')"),
    ('{"heap-allocated key": [], []: [1]}', "cannot use 'list' as a dict key (unhashable type: 'list')"),
):
    try:
        literal_eval(source)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == message
