import csv
import sys

if sys.platform != 'monty':
    assert sys.version_info[:2] == (3, 14)


class Sink:
    def __init__(self):
        self.lines = []

    def write(self, text):
        self.lines.append(text)
        return len(text)


class NumberByInt:
    def __int__(self):
        raise AssertionError('numeric conversion must not run')

    def __str__(self):
        return '7'


class NumberByFloat:
    def __float__(self):
        raise AssertionError('numeric conversion must not run')

    def __str__(self):
        return '7'


class NumberByIndex:
    def __index__(self):
        raise AssertionError('numeric conversion must not run')

    def __str__(self):
        return '7'


def row(value, **kwargs):
    sink = Sink()
    csv.writer(sink, **kwargs).writerow([value])
    return sink.lines


for value in (NumberByInt(), NumberByFloat(), NumberByIndex()):
    assert row(value, quoting=csv.QUOTE_NONNUMERIC) == ['7\r\n']

for hook in ('__int__', '__float__', '__index__'):
    for member in (None, 1):
        namespace = {'__str__': lambda self: '7', hook: member}
        number = type('Number', (), namespace)()
        assert row(number, quoting=csv.QUOTE_NONNUMERIC) == ['7\r\n']


for name in ('__complex__', '__add__', '__bool__', '__len__'):
    namespace = {'__str__': lambda self: '7', name: lambda *args: None}
    excluded = type('Excluded', (), namespace)()
    assert row(excluded, quoting=csv.QUOTE_NONNUMERIC) == ['"7"\r\n']


class InstanceOnly:
    def __str__(self):
        return '7'


instance_only = InstanceOnly()
instance_only.__int__ = lambda: 7
assert row(instance_only, quoting=csv.QUOTE_NONNUMERIC) == ['"7"\r\n']


class MutableNumber:
    def __str__(self):
        return '7'


mutable = MutableNumber()
sink = Sink()
writer = csv.writer(sink, quoting=csv.QUOTE_NONNUMERIC)
writer.writerow([mutable])
MutableNumber.__int__ = None
writer.writerow([mutable])
MutableNumber.__int__ = 1
writer.writerow([mutable])
assert sink.lines == ['"7"\r\n', '7\r\n', '7\r\n']

DynamicNumber = type('DynamicNumber', (), {'__int__': None, '__str__': lambda self: '7'})
assert row(DynamicNumber(), quoting=csv.QUOTE_NONNUMERIC) == ['7\r\n']


class AddHookDuringString:
    def __str__(self):
        AddHookDuringString.__int__ = None
        return '7'


mutating = AddHookDuringString()
sink = Sink()
csv.writer(sink, quoting=csv.QUOTE_NONNUMERIC).writerow([mutating, mutating])
assert sink.lines == ['"7",7\r\n']


class DelimitedNumber:
    def __int__(self):
        raise AssertionError('numeric conversion must not run')

    def __str__(self):
        return '7,8'


assert row(DelimitedNumber(), quoting=csv.QUOTE_NONNUMERIC) == ['"7,8"\r\n']

sink = Sink()
csv.DictWriter(sink, ['number'], quoting=csv.QUOTE_NONNUMERIC).writerow({'number': NumberByInt()})
assert sink.lines == ['7\r\n']

assert row(12, quoting=csv.QUOTE_NONNUMERIC) == ['12\r\n']
assert row(12, quoting=csv.QUOTE_MINIMAL) == ['12\r\n']
assert row(NumberByInt(), quoting=csv.QUOTE_MINIMAL) == ['7\r\n']
