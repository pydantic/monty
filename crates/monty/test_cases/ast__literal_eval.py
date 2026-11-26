import ast
from ast import literal_eval

assert ast.literal_eval(node_or_string=' \t[True, False, None, ...]') == [True, False, None, ...]
assert literal_eval(r"""['one' "two", b'\x00' b'abc', '\u00e9']""") == ['onetwo', b'\x00abc', 'é']
assert literal_eval("[(+3, -2.5), {True: 1, 1: 2}, {'a', 'b', 'a'}]") == [(3, -2.5), {True: 2}, {'a', 'b'}]
assert literal_eval('1,') == (1,)
assert literal_eval('()') == ()
assert literal_eval('{}') == {}
assert literal_eval('-0x1_0000_0000_0000_0000') == -(2**64)
assert literal_eval('+12_345_678_901_234_567_890') == 12_345_678_901_234_567_890
assert literal_eval('[-2 + 3j, +4.5 - 2j, -3j, +1j]') == [-2 + 3j, 4.5 - 2j, -3j, 1j]
assert repr(literal_eval('-0j')) == '(-0-0j)'
assert literal_eval('\n# comment\n(1,\n2)\n') == (1, 2)
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

for source in ('[1,', '1; 2', '\0', '\ufeff1', '[' * 201 + '0' + ']' * 201):
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
