import csv
import sys

is_monty = sys.platform == 'monty'


def rd(lines, **kw):
    return list(csv.reader(lines, **kw))


# === Basic parsing ===
assert rd(['a,b,c', '1,2,3']) == [['a', 'b', 'c'], ['1', '2', '3']]
assert rd([]) == []
# Line terminators are consumed, whatever their style.
assert rd(['a,b\n', '1,2\r\n', 'x\r']) == [['a', 'b'], ['1', '2'], ['x']]
assert rd(['a\r\n']) == [['a']]
assert rd(['a\n\n']) == [['a']]
# An empty line is an empty record.
assert rd(['', 'a', '\n']) == [[], ['a'], []]
assert rd(['a,', ',']) == [['a', ''], ['', '']]
# Whitespace is data unless skipinitialspace is set.
assert rd([' a, b ,"c" ']) == [[' a', ' b ', 'c ']]
assert rd([' a, b ,  "c" '], skipinitialspace=True) == [['a', 'b ', 'c ']]
assert rd(['a,  "b c"'], skipinitialspace=True) == [['a', 'b c']]
assert rd(['a,  ,b'], skipinitialspace=True) == [['a', '', 'b']]
assert rd([' "a"']) == [[' "a"']]
assert rd([' "a",  "b"'], skipinitialspace=True) == [['a', 'b']]
# Any iterable of strings works, including a generator and a string's characters.
assert rd(x for x in ['a,b']) == [['a', 'b']]
assert rd('ab') == [['a'], ['b']]
assert rd(['a€b'], delimiter='€') == [['a', 'b']]
assert rd(['a  b'], delimiter=' ', skipinitialspace=True) == [['a', 'b']]

# === Quoting ===
assert rd(['"a,b",c', '"x""y",z', '"multi', 'line",q']) == [['a,b', 'c'], ['x"y', 'z'], ['multiline', 'q']]
assert rd(['""']) == [['']]
assert rd(['"']) == [['']]
assert rd(['"\n"']) == [['\n']]
# A quote inside an unquoted field is data; after a quoted field it is appended unless strict.
assert rd(['a"b"c']) == [['a"b"c']]
assert rd(['"ab"c"d"']) == [['abc"d"']]
assert rd(['"a"b,c']) == [['ab', 'c']]
# Unterminated quotes close at end of input unless strict.
assert rd(['"abc']) == [['abc']]
assert rd(['"abc', 'd']) == [['abcd']]
assert rd(['"a""b"'], doublequote=False) == [['a"b"']]
assert rd(['"a\\"b"'], doublequote=False, escapechar='\\') == [['a"b']]
assert rd(['a,"b",c'], quoting=csv.QUOTE_NONE) == [['a', '"b"', 'c']]
assert rd(['a"b,c'], quoting=csv.QUOTE_NONE) == [['a"b', 'c']]
assert rd(['a,b'], quotechar=None, quoting=csv.QUOTE_NONE) == [['a', 'b']]
assert rd(['a\tb,c'], dialect='excel-tab') == [['a', 'b,c']]

# === Escaping ===
assert rd(['a\\,b,c', 'x\\"y'], escapechar='\\') == [['a,b', 'c'], ['x"y']]
assert rd(['"a\\"b",c'], escapechar='\\') == [['a"b', 'c']]
# Without an escapechar a backslash is data.
assert rd(['a\\,b']) == [['a\\', 'b']]
# An escape at the end of a line continues the field onto the next one.
assert rd(['a\\', 'b'], escapechar='\\') == [['a\nb']]
assert rd(['a\\\r\n', 'b']) == [['a\\'], ['b']]

# === QUOTE_NONNUMERIC, QUOTE_NOTNULL and QUOTE_STRINGS on read ===
assert rd(['1,,2'], quoting=csv.QUOTE_NONNUMERIC) == [[1.0, '', 2.0]]
assert rd([' 1.5 ,1e3,inf,-0,1_000'], quoting=csv.QUOTE_NONNUMERIC) == [[1.5, 1000.0, float('inf'), -0.0, 1000.0]]
assert rd(['"a\n",1'], quoting=csv.QUOTE_NONNUMERIC) == [['a\n', 1.0]]
assert rd(['a,,"",b'], quoting=csv.QUOTE_NOTNULL) == [['a', None, '', 'b']]
assert rd(['1,,"",2'], quoting=csv.QUOTE_STRINGS) == [[1.0, None, '', 2.0]]
for quoting, line in ((csv.QUOTE_NONNUMERIC, 'abc'), (csv.QUOTE_STRINGS, '1,x')):
    try:
        rd([line], quoting=quoting)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == "could not convert string to float: '" + line.split(',')[-1] + "'"

# === Errors while parsing ===
try:
    rd(['"a"b,c'], strict=True)
    assert False, 'expected csv.Error'
except csv.Error as exc:
    assert str(exc) == "',' expected after '\"'"
for lines in (['"abc'], ['"'], ['"a', 'b']):
    try:
        rd(lines, strict=True)
        assert False, 'expected csv.Error'
    except csv.Error as exc:
        assert str(exc) == 'unexpected end of data'
for line in ('a\nb,c', 'a\rb'):
    try:
        rd([line])
        assert False, 'expected csv.Error'
    except csv.Error as exc:
        assert str(exc) == "new-line character seen in unquoted field - do you need to open the file with newline=''?"
for item, name in ((1, 'int'), (b'a,b', 'bytes'), (None, 'NoneType')):
    try:
        rd([item])
        assert False, 'expected csv.Error'
    except csv.Error as exc:
        assert str(exc) == 'iterator should return strings, not ' + name + ' (the file should be opened in text mode)'
try:
    rd(['a' * 131073])
    assert False, 'expected csv.Error'
except csv.Error as exc:
    assert str(exc) == 'field larger than field limit (131072)'
assert len(rd(['a' * 131072])[0][0]) == 131072
try:
    list(csv.reader(5))
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"

# === The reader object ===
r = csv.reader(['a', '"b', 'c"', 'd'])
assert iter(r) is r
assert r.line_num == 0
assert next(r) == ['a']
assert r.line_num == 1
assert next(r) == ['bc']
assert r.line_num == 3
assert next(r) == ['d']
assert r.line_num == 4
try:
    next(r)
    assert False, 'expected StopIteration'
except StopIteration:
    pass
assert next(csv.reader([]), 'default') == 'default'
assert str(type(r)) == "<class '_csv.reader'>"
assert repr(r).startswith('<_csv.reader object at 0x')
assert type(r.dialect) is type(csv.get_dialect('excel'))
assert csv.reader([], delimiter=';').dialect.delimiter == ';'
assert csv.reader([], dialect='excel-tab').dialect.delimiter == '\t'
assert csv.reader([], 'excel-tab').dialect.delimiter == '\t'
assert csv.reader([], dialect=csv.excel_tab).dialect.delimiter == '\t'
assert csv.reader([], dialect=csv.get_dialect('excel-tab'), delimiter=';').dialect.delimiter == ';'
# The reader's attributes are read-only, and it takes no new ones.
for name in ('line_num', 'dialect'):
    try:
        setattr(r, name, 1)
        assert False, 'expected AttributeError'
    except AttributeError as exc:
        assert str(exc) == 'readonly attribute'
try:
    r.nope = 1
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'_csv.reader' object has no attribute 'nope' and no __dict__ for setting new attributes"


# Any object with the attributes serves as a dialect; missing ones take the defaults.
class Semicolons:
    delimiter = ';'


assert rd(['a;b'], dialect=Semicolons) == [['a', 'b']]
assert rd(['a;b'], dialect=Semicolons()) == [['a', 'b']]
assert csv.reader([], dialect=Semicolons).dialect.quotechar == '"'
assert csv.reader([], dialect=5).dialect.delimiter == ','

# === Format parameter validation ===
for kwargs, message in (
    ({'delimiter': 'ab'}, '"delimiter" must be a unicode character, not a string of length 2'),
    ({'delimiter': ''}, '"delimiter" must be a unicode character, not a string of length 0'),
    ({'delimiter': 5}, '"delimiter" must be a unicode character, not int'),
    ({'delimiter': None}, '"delimiter" must be a unicode character, not NoneType'),
    ({'quotechar': 'ab'}, '"quotechar" must be a unicode character or None, not a string of length 2'),
    ({'quotechar': 5}, '"quotechar" must be a unicode character or None, not int'),
    ({'escapechar': 'ab'}, '"escapechar" must be a unicode character or None, not a string of length 2'),
    ({'quoting': 9}, 'bad "quoting" value'),
    ({'quoting': 'x'}, '"quoting" must be an integer, not str'),
    ({'quoting': True}, '"quoting" must be an integer, not bool'),
    ({'quoting': 1.0}, '"quoting" must be an integer, not float'),
    ({'lineterminator': 5}, '"lineterminator" must be a string, not int'),
    ({'lineterminator': None}, '"lineterminator" must be a string, not NoneType'),
    ({'quotechar': None, 'quoting': csv.QUOTE_MINIMAL}, 'quotechar must be set if quoting enabled'),
    ({'dialect': type('Bad', (), {'delimiter': 5})}, '"delimiter" must be a unicode character, not int'),
):
    try:
        csv.reader([], **kwargs)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == message
for kwargs, message in (
    ({'escapechar': ','}, 'bad delimiter or escapechar value'),
    ({'quotechar': ','}, 'bad delimiter or quotechar value'),
    ({'escapechar': '"'}, 'bad escapechar or quotechar value'),
    ({'delimiter': '\n'}, 'bad delimiter value'),
    ({'quotechar': '\r'}, 'bad quotechar value'),
    ({'escapechar': '\n'}, 'bad escapechar value'),
    ({'delimiter': ';', 'lineterminator': ';'}, 'bad delimiter or lineterminator value'),
    ({'escapechar': ' ', 'skipinitialspace': True}, 'bad escapechar value'),
):
    try:
        csv.reader([], **kwargs)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == message
# Spaces are fine as delimiter and, without skipinitialspace, as escape or quote.
assert csv.reader([], delimiter=' ').dialect.delimiter == ' '
assert csv.reader([], escapechar=' ').dialect.escapechar == ' '
# Booleans go through bool().
assert csv.reader([], doublequote='yes').dialect.doublequote is True
assert csv.reader([], strict=[1]).dialect.strict is True
assert csv.reader([], skipinitialspace=0).dialect.skipinitialspace is False
# An explicit quotechar=None without quoting selects QUOTE_NONE.
assert csv.reader([], quotechar=None).dialect.quoting == csv.QUOTE_NONE
assert csv.reader([], quoting=csv.QUOTE_NONE).dialect.quotechar == '"'
assert csv.reader([], escapechar=None).dialect.escapechar is None
assert csv.reader([], lineterminator='').dialect.lineterminator == ''
try:
    csv.reader([], dialect='nope')
    assert False, 'expected csv.Error'
except csv.Error as exc:
    assert str(exc) == 'unknown dialect'

# === Call signature ===
try:
    csv.reader()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'reader expected at least 1 argument, got 0'
try:
    csv.reader([], 'excel', 'x')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'reader expected at most 2 arguments, got 3'
try:
    csv.reader([], foo=1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "this function got an unexpected keyword argument 'foo'"
