import csv
import sys
from io import StringIO

reader = csv.reader(['a,b'])
line_num = type(reader).line_num
if sys.platform == 'monty':
    assert type(line_num).__name__ == 'getset_descriptor'
    assert repr(line_num) == "<attribute 'line_num' of '_csv.reader' objects>"
else:
    assert type(line_num).__name__ == 'member_descriptor'
    assert repr(line_num) == "<member 'line_num' of '_csv.reader' objects>"
assert line_num.__name__ == 'line_num'
assert line_num.__objclass__ is type(reader)
assert line_num.__get__(None, type(reader)) is line_num
assert line_num.__get__(reader) == 0
assert next(reader) == ['a', 'b']
assert line_num.__get__(reader) == reader.line_num == 1

writer = csv.writer(StringIO())
assert type(writer).dialect.__get__(writer).delimiter == ','
try:
    line_num.__get__(writer)
except TypeError:
    pass
else:
    assert False

dialect = csv.get_dialect('excel')
assert type(dialect).__name__ == 'Dialect'
assert type(dialect).delimiter.__get__(dialect) == ','
try:
    dialect.delimiter = []
except AttributeError:
    pass
else:
    assert False

rows = csv.DictReader(['a,b', 'c,d'])
fieldnames = csv.DictReader.fieldnames
assert rows.line_num == 0
assert fieldnames.__get__(rows) == ['a', 'b']
assert rows.line_num == 1
fieldnames.__set__(rows, ['x', 'y'])
assert rows.fieldnames == ['x', 'y']
rows.fieldnames = None
assert rows.fieldnames == ['c', 'd']
assert rows.line_num == 2
rows.fieldnames = lambda arg: arg
assert rows.fieldnames('call through the getter') == 'call through the getter'

try:
    reader.line_num([])
except TypeError:
    pass
else:
    assert False
