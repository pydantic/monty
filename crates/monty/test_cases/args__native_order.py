import datetime
import sys

calls = []


class Index:
    def __init__(self, name, value):
        self.name = name
        self.value = value

    def __index__(self):
        calls.append(self.name)
        if self.value is None:
            raise ValueError('index conversion failed')
        return self.value


# Keyword order does not control protocol conversion order.
date = datetime.date(day=Index('day', 1), month=Index('month', 2), year=Index('year', 2026))
assert date == datetime.date(2026, 2, 1)
assert calls == ['year', 'month', 'day']
calls.clear()

# Monty checks the call shape before any __index__ invocation.
try:
    datetime.date(Index('year', 2026), Index('month', 2), bogus=1)
except TypeError as exc:
    if sys.platform == 'monty':
        assert 'bogus' in str(exc)
        assert calls == []
else:
    assert False, 'expected an invalid call shape to raise TypeError'
calls.clear()

# A failed conversion stops later conversions.
try:
    datetime.date(Index('year', 2026), Index('month', None), Index('day', 1))
except ValueError as exc:
    assert str(exc) == 'index conversion failed'
else:
    assert False, 'expected the conversion error to propagate'
assert calls == ['year', 'month']
calls.clear()

# Calendar validation runs after all three arguments have been converted.
try:
    datetime.date(Index('year', 2026), Index('month', 13), Index('day', 1))
except ValueError as exc:
    assert 'month' in str(exc)
else:
    assert False, 'expected invalid month to raise ValueError'
assert calls == ['year', 'month', 'day']
