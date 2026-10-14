import csv
import sys

is_monty = sys.platform == 'monty'


class Sink:
    """Collects what the writer hands to write(), standing in for a file."""

    def __init__(self):
        self.lines = []

    def write(self, text):
        self.lines.append(text)
        return len(text)


class Rows:
    """An iterator of rows that raises in place of a `None` row."""

    def __init__(self, rows):
        self.rows = iter(rows)

    def __iter__(self):
        return self

    def __next__(self):
        row = next(self.rows)
        if row is None:
            raise ValueError('boom')
        return row


def wr(rows, **kw):
    sink = Sink()
    writer = csv.writer(sink, **kw)
    for row in rows:
        writer.writerow(row)
    return ''.join(sink.lines)


# === Basic formatting ===
assert wr([['a', 'b'], [1, 2.5, None, True]]) == 'a,b\r\n1,2.5,,True\r\n'
assert wr([['a,b', 'c"d', 'e\nf', ' g', '']]) == '"a,b","c""d","e\nf", g,\r\n'
assert wr([[]]) == '\r\n'
assert wr([['']]) == '""\r\n'
assert wr([[None]]) == '""\r\n'
assert (
    wr([[1e20, 0.1, 1 / 3, float('inf'), -0.0, 10**20]])
    == '1e+20,0.1,0.3333333333333333,inf,-0.0,100000000000000000000\r\n'
)
assert wr([[object]]) == "<class 'object'>\r\n"
assert wr([[[1, 2]]]) == '"[1, 2]"\r\n'
assert wr([['a']], lineterminator='\n') == 'a\n'
assert wr([['a', 1]], dialect='unix') == '"a","1"\n'
assert wr([['a', 'b\tc']], dialect='excel-tab') == 'a\t"b\tc"\r\n'
assert wr([['a b', 'c']], delimiter=' ') == '"a b" c\r\n'
assert wr([[' a']], delimiter=' ') == '" a"\r\n'
# Strings are iterables of characters; dicts iterate their keys.
assert wr(['ab']) == 'a,b\r\n'
assert wr([{'a': 1}]) == 'a\r\n'

# === Quoting modes ===
assert wr([['a', 1, None]], quoting=csv.QUOTE_ALL) == '"a","1",""\r\n'
assert wr([['']], quoting=csv.QUOTE_ALL) == '""\r\n'
assert wr([['a', 1, 2.5, None, True, '']], quoting=csv.QUOTE_NONNUMERIC) == '"a",1,2.5,"",True,""\r\n'
assert wr([[1j, 10**30]], quoting=csv.QUOTE_NONNUMERIC) == '1j,1000000000000000000000000000000\r\n'
assert wr([[None]], quoting=csv.QUOTE_NONNUMERIC) == '""\r\n'
assert wr([['a', 1, None, '']], quoting=csv.QUOTE_NOTNULL) == '"a","1",,""\r\n'
assert wr([['a', 1, None, '']], quoting=csv.QUOTE_STRINGS) == '"a",1,,""\r\n'
for quoting, row in ((csv.QUOTE_NONE, ['']), (csv.QUOTE_STRINGS, [None]), (csv.QUOTE_NOTNULL, [None])):
    try:
        wr([row], quoting=quoting)
        assert False, 'expected csv.Error'
    except csv.Error as exc:
        assert str(exc) == 'single empty field record must be quoted'

# === Line terminator characters inside fields ===
assert wr([['a\r\nb']]) == '"a\r\nb"\r\n'
assert wr([['a;b']], lineterminator=';') == '"a;b";'
assert wr([['a\nb', 'c\rd']], lineterminator=';') == '"a\nb","c\rd";'

# === Escaping ===
assert wr([['a,b', 'c"d', 'e\\f', 'g\nh']], quoting=csv.QUOTE_NONE, escapechar='\\') == 'a\\,b,c\\"d,e\\\\f,g\\\nh\r\n'
assert wr([['a,b', 'c"d']], quoting=csv.QUOTE_NONE, escapechar='\\', quotechar=None) == 'a\\,b,c"d\r\n'
assert wr([['a\nb', 'c\rd']], quoting=csv.QUOTE_NONE, escapechar='\\') == 'a\\\nb,c\\\rd\r\n'
assert wr([['c"d']], doublequote=False, escapechar='\\') == 'c\\"d\r\n'
assert wr([['a\\b', 'x']], escapechar='\\') == 'a\\\\b,x\r\n'
assert wr([['a\\"b']], escapechar='\\') == '"a\\\\""b"\r\n'
for kwargs, row in (
    ({'quoting': csv.QUOTE_NONE}, ['a,b']),
    ({'quoting': csv.QUOTE_NONE}, ['a\nb']),
    ({'quoting': csv.QUOTE_NONE, 'lineterminator': ';'}, ['a\nb']),
    ({'doublequote': False}, ['c"d']),
    ({'delimiter': '\t', 'quoting': csv.QUOTE_NONE}, ['a\tb']),
):
    try:
        wr([row], **kwargs)
        assert False, 'expected csv.Error'
    except csv.Error as exc:
        assert str(exc) == 'need to escape, but no escapechar set'

# === writerow / writerows ===
sink = Sink()
writer = csv.writer(sink)
assert str(type(writer)) == "<class '_csv.writer'>"
assert repr(writer).startswith('<_csv.writer object at 0x')
assert writer.dialect.lineterminator == '\r\n'
assert csv.writer(sink, delimiter=';').dialect.delimiter == ';'
assert csv.writer(sink, 'unix').dialect.lineterminator == '\n'
# The writer's dialect is read-only, and it takes no new attributes.
try:
    writer.dialect = csv.excel
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == 'readonly attribute'
try:
    writer.nope = 1
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'_csv.writer' object has no attribute 'nope' and no __dict__ for setting new attributes"
# writerow returns what write() returned; writerows returns None and calls write() per row.
assert writer.writerow(['a', 'b']) == 5
assert writer.writerows([['c'], ['d']]) is None
assert writer.writerows([]) is None
assert writer.writerows(x for x in [['e']]) is None
assert sink.lines == ['a,b\r\n', 'c\r\n', 'd\r\n', 'e\r\n']
# Each row is written as it arrives, so the rows before a failing one stay written.
sink = Sink()
writer = csv.writer(sink)
try:
    writer.writerows(Rows([['a'], None, ['b']]))
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'boom'
assert sink.lines == ['a\r\n']
for row, name in ((5, 'int'), (None, 'NoneType')):
    try:
        writer.writerow(row)
        assert False, 'expected csv.Error'
    except csv.Error as exc:
        assert str(exc) == 'iterable expected, not ' + name
try:
    writer.writerows(5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"
try:
    writer.writerow()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'writer.writerow() takes exactly one argument (0 given)'
try:
    writer.writerow([], [])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'writer.writerow() takes exactly one argument (2 given)'

# === Constructor ===
try:
    csv.writer(5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'argument 1 must have a "write" method'


# write must be callable, and is bound at construction: rebinding it later changes nothing.
class NotCallable:
    write = 1


try:
    csv.writer(NotCallable())
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'argument 1 must have a "write" method'
sink = Sink()
writer = csv.writer(sink)
sink.write = lambda text: None
assert writer.writerow(['a']) == 3
assert sink.lines == ['a\r\n']
try:
    csv.writer()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'writer expected at least 1 argument, got 0'
for quoting in (2**31, 2**40):
    try:
        csv.writer(sink, quoting=quoting)
        assert False, 'expected OverflowError'
    except OverflowError as exc:
        assert str(exc) == 'Python int too large to convert to C int'
try:
    csv.writer(sink, 'excel', 'x')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'writer expected at most 2 arguments, got 3'
try:
    csv.writer(sink, nope=1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "this function got an unexpected keyword argument 'nope'"
try:
    csv.writer(sink, delimiter='ab')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == '"delimiter" must be a unicode character, not a string of length 2'
