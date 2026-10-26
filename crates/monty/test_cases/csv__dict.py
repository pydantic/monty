import csv
import sys

is_monty = sys.platform == 'monty'


class Sink:
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


def dr(lines, *args, **kwargs):
    reader = csv.DictReader(lines, *args, **kwargs)
    return list(reader), reader.fieldnames, reader.line_num


# === DictReader ===
assert dr(['a,b', '1,2', '3,4']) == ([{'a': '1', 'b': '2'}, {'a': '3', 'b': '4'}], ['a', 'b'], 3)
# Short rows are padded with restval; long rows put the surplus under restkey.
assert dr(['a,b', '1', '1,2,3,4']) == ([{'a': '1', 'b': None}, {'a': '1', 'b': '2', None: ['3', '4']}], ['a', 'b'], 3)
assert dr(['a,b', '1', '1,2,3,4'], restkey='extra', restval='X') == (
    [{'a': '1', 'b': 'X'}, {'a': '1', 'b': '2', 'extra': ['3', '4']}],
    ['a', 'b'],
    3,
)
assert dr(['a,b,c', '1']) == ([{'a': '1', 'b': None, 'c': None}], ['a', 'b', 'c'], 2)
assert dr(['a', '1,2,3']) == ([{'a': '1', None: ['2', '3']}], ['a'], 2)
# fieldnames given up front means the first row is data; any iterable of keys works.
assert dr(['1,2', '3,4'], fieldnames=['x', 'y']) == ([{'x': '1', 'y': '2'}, {'x': '3', 'y': '4'}], ['x', 'y'], 2)
assert dr(['1,2'], ('x', 'y')) == ([{'x': '1', 'y': '2'}], ('x', 'y'), 1)
assert dr(['1,2'], fieldnames='xy') == ([{'x': '1', 'y': '2'}], 'xy', 1)
# An iterator of names is read once into a list.
assert dr(['1,2'], fieldnames=iter(['x', 'y'])) == ([{'x': '1', 'y': '2'}], ['x', 'y'], 1)
# Positional restkey, restval and dialect, plus format parameters.
assert dr(['1,2,3'], ['x', 'y'], 'r', 'v', 'excel-tab') == ([{'x': '1,2,3', 'y': 'v'}], ['x', 'y'], 1)
assert dr(['a;b', '1;2'], delimiter=';') == ([{'a': '1', 'b': '2'}], ['a', 'b'], 2)
# Blank lines are skipped, but a blank header line is an empty header.
assert dr(['a,b', '', '1,2', '', '']) == ([{'a': '1', 'b': '2'}], ['a', 'b'], 5)
assert dr(['', 'a,b', '1,2']) == ([{None: ['a', 'b']}, {None: ['1', '2']}], [], 3)
assert dr([]) == ([], None, 0)
# Duplicate names keep the last value.
assert dr(['a,a', '1,2']) == ([{'a': '2'}], ['a', 'a'], 2)
assert dr(['a,a', '1,2', '3,4']) == ([{'a': '2'}, {'a': '4'}], ['a', 'a'], 3)

reader = csv.DictReader(['a,b', '', '1,2'])
assert iter(reader) is reader
assert str(type(reader)) == "<class 'csv.DictReader'>"
assert repr(reader).startswith('<csv.DictReader object at 0x')
assert reader.line_num == 0
# Accessing fieldnames reads the header.
assert reader.fieldnames == ['a', 'b']
assert reader.line_num == 1
assert next(reader) == {'a': '1', 'b': '2'}
assert reader.line_num == 3
try:
    next(reader)
    assert False, 'expected StopIteration'
except StopIteration:
    pass
assert str(type(reader.reader)) == "<class '_csv.reader'>"
assert reader.restkey is None
assert reader.restval is None
assert reader.dialect == 'excel'
assert csv.DictReader([], dialect='unix').dialect == 'unix'
assert type(dr(['a', '1'])[0][0]) is dict

# fieldnames, restkey, restval and dialect can be assigned.
reader = csv.DictReader(['1,2', '3'])
reader.fieldnames = ['p', 'q']
reader.restval = 'Z'
assert list(reader) == [{'p': '1', 'q': '2'}, {'p': '3', 'q': 'Z'}]
reader.dialect = 'unix'
assert reader.dialect == 'unix'
# fieldnames = None unsets them, so the next row read is the header.
reader = csv.DictReader(['a,b', '1,2'], fieldnames=['x', 'y'])
reader.fieldnames = None
assert reader.fieldnames == ['a', 'b']
assert list(reader) == [{'a': '1', 'b': '2'}]
# A non-iterable fieldnames is rejected at construction; an explicit None is the header-to-come.
try:
    csv.DictReader(['1,2'], fieldnames=5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"
assert csv.DictReader(['a,b', '1,2'], fieldnames=None).fieldnames == ['a', 'b']
try:
    next(csv.DictReader([], fieldnames=['a']))
    assert False, 'expected StopIteration'
except StopIteration:
    pass
try:
    csv.DictReader()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "DictReader.__init__() missing 1 required positional argument: 'f'"
try:
    csv.DictReader([], nope=1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "this function got an unexpected keyword argument 'nope'"


# === DictWriter ===
def dw(rows, fieldnames, **kwargs):
    sink = Sink()
    writer = csv.DictWriter(sink, fieldnames, **kwargs)
    for row in rows:
        writer.writerow(row)
    return ''.join(sink.lines)


assert dw([{'a': 1, 'b': 2}, {'b': 3}], ['a', 'b']) == '1,2\r\n,3\r\n'
assert dw([{'a': 1}], ['a', 'b'], restval='-') == '1,-\r\n'
assert dw([{'a': 1, 'c': 2}], ['a', 'b'], extrasaction='ignore') == '1,\r\n'
assert dw([{'a': 1, 'b': 2}], ['a', 'b'], delimiter=';') == '1;2\r\n'
assert dw([{'a': 1}], 'ab') == '1,\r\n'
assert dw([{1: 'x', 2: 'y'}], [1, 2]) == 'x,y\r\n'
try:
    dw([{'a': 1, 'zz': 2}], ['a'])
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == "dict contains fields not in fieldnames: 'zz'"


# An empty repr still separates the offending keys.
class Blank:
    def __repr__(self):
        return ''


try:
    dw([{Blank(): 1, Blank(): 2}], [])
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'dict contains fields not in fieldnames: , '
try:
    csv.DictWriter(Sink(), ['a'], extrasaction='nope')
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == "extrasaction (nope) must be 'raise' or 'ignore'"
# extrasaction is stored lower-cased.
assert csv.DictWriter(Sink(), ['a'], extrasaction='IGNORE').extrasaction == 'ignore'
assert csv.DictWriter(Sink(), ['a'], '-', 'ignore', 'excel-tab').extrasaction == 'ignore'
# A row that is not a dict fails on the method the check reaches first.
for extrasaction, method in (('raise', 'keys'), ('ignore', 'get')):
    try:
        dw([[1, 2]], ['a', 'b'], extrasaction=extrasaction)
        assert False, 'expected AttributeError'
    except AttributeError as exc:
        assert str(exc) == "'list' object has no attribute '" + method + "'"

sink = Sink()
writer = csv.DictWriter(sink, ('a', 'b'))
assert str(type(writer)) == "<class 'csv.DictWriter'>"
assert writer.fieldnames == ('a', 'b')
assert writer.restval == ''
assert writer.extrasaction == 'raise'
assert str(type(writer.writer)) == "<class '_csv.writer'>"
# writeheader and writerow return what write() returned; writerows returns None.
assert writer.writeheader() == 5
assert writer.writerow({'a': 1}) == 4
assert writer.writerows([{'b': 2}, {'a': 3, 'b': 4}]) is None
assert writer.writerows({'a': i} for i in range(2)) is None
assert sink.lines == ['a,b\r\n', '1,\r\n', ',2\r\n', '3,4\r\n', '0,\r\n', '1,\r\n']
# Each dict is written as it arrives, so the rows before a failing one stay written.
sink = Sink()
writer = csv.DictWriter(sink, ['a'])
try:
    writer.writerows(Rows([{'a': 1}, None, {'a': 2}]))
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'boom'
assert sink.lines == ['1\r\n']
# fieldnames, restval and extrasaction are plain attributes, read at each write.
sink = Sink()
writer = csv.DictWriter(sink, ['a'])
writer.fieldnames = ['a', 'b']
writer.restval = '-'
writer.extrasaction = 'ignore'
assert writer.writerow({'a': 1, 'c': 3}) == 5
assert sink.lines == ['1,-\r\n']


# writerows reads them again for each row, so the row iterator can change them, even by
# mutating the fieldnames list in place.
class Changing:
    """Rows that widen, then replace, the writer's fieldnames between rows."""

    def __init__(self, writer):
        self.writer = writer
        self.n = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.n += 1
        if self.n == 1:
            return {'a': 1}
        if self.n == 2:
            self.writer.fieldnames.append('b')
            return {'a': 2, 'b': 3}
        if self.n == 3:
            self.writer.fieldnames = ['b']
            self.writer.restval = '-'
            self.writer.extrasaction = 'ignore'
            return {'a': 4}
        raise StopIteration


sink = Sink()
writer = csv.DictWriter(sink, ['a'])
writer.writerows(Changing(writer))
assert sink.lines == ['1\r\n', '2,3\r\n', '-\r\n']
# Nothing is read from fieldnames until a row needs it.
writer = csv.DictWriter(Sink(), [['a']])
assert writer.writerows([]) is None
# A row that is not a dict fails before the names are hashed under 'raise' ...
try:
    writer.writerow(1)
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'int' object has no attribute 'keys'"


# ... and after fieldnames is iterated under 'ignore'.
class OnceNames:
    """Iterable once (at construction), then refuses."""

    def __init__(self):
        self.calls = 0

    def __iter__(self):
        self.calls += 1
        if self.calls > 1:
            raise TypeError('names gone')
        return iter(['a'])


try:
    csv.DictWriter(Sink(), OnceNames(), extrasaction='ignore').writerow(1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'names gone'
# The check for keys outside fieldnames puts the names in a set, so they must be hashable.
try:
    csv.DictWriter(Sink(), [['a']]).writerow({'a': 1})
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "cannot use 'list' as a set element (unhashable type: 'list')"
# writeheader builds a dict of the names first, which rejects an unhashable one under
# either extrasaction and collapses a repeated name.
for extrasaction in ('raise', 'ignore'):
    try:
        csv.DictWriter(Sink(), [['a']], extrasaction=extrasaction).writeheader()
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == "cannot use 'list' as a dict key (unhashable type: 'list')"
sink = Sink()
csv.DictWriter(sink, ['a', 'a', 'b']).writeheader()
assert sink.lines == ['a,a,b\r\n']
# An iterator of names is read once into a list at construction, so every row sees it ...
sink = Sink()
writer = csv.DictWriter(sink, iter(['a', 'b']))
writer.writeheader()
writer.writerow({'a': 1, 'b': 2})
writer.writerow({'a': 3, 'b': 4})
assert sink.lines == ['a,b\r\n', '1,2\r\n', '3,4\r\n']
assert writer.fieldnames == ['a', 'b']
# ... a non-iterable is rejected there, and an explicit None only fails once a row needs it.
try:
    csv.DictWriter(Sink(), 5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"
writer = csv.DictWriter(Sink(), None)
assert writer.fieldnames is None
try:
    writer.writeheader()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'NoneType' object is not iterable"
try:
    csv.DictWriter(sink)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "DictWriter.__init__() missing 1 required positional argument: 'fieldnames'"
try:
    csv.DictWriter(sink, ['a'], nope=1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "this function got an unexpected keyword argument 'nope'"
try:
    csv.DictWriter(5, ['a'])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'argument 1 must have a "write" method'
try:
    dw([{'a': 1}], 5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"
