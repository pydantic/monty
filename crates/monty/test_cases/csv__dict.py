import csv
import sys

is_monty = sys.platform == 'monty'


class Sink:
    def __init__(self):
        self.lines = []

    def write(self, text):
        self.lines.append(text)
        return len(text)


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

# fieldnames, restkey and restval can be assigned.
reader = csv.DictReader(['1,2', '3'])
reader.fieldnames = ['p', 'q']
reader.restval = 'Z'
assert list(reader) == [{'p': '1', 'q': '2'}, {'p': '3', 'q': 'Z'}]
try:
    list(csv.DictReader(['1,2'], fieldnames=5))
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"
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
