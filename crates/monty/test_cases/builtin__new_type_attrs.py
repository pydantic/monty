from annotationlib import ForwardRef
from datetime import date
from decimal import Context, Decimal, Inexact, getcontext, setcontext

setcontext(Context())
value = Decimal('12.5')
assert value.adjusted() == Decimal.adjusted(value) == 1
assert Decimal.quantize(value, Decimal('1')) == Decimal('12')
assert Decimal.from_float(0.5) == Decimal('0.5')
assert value.from_float(0.5) == Decimal('0.5')
assert Decimal.from_number(5) == Decimal(5)
assert Decimal.as_tuple(value) == value.as_tuple()
assert value.__copy__() is value
context = Context(prec=4)
assert Context.add(context, value, Decimal('1.5')) == Decimal('14')
assert context.create_decimal('1.234567') == Decimal('1.235')
assert context.Etiny() == Context.Etiny(context)
assert context.Etop() == Context.Etop(context)
assert context.__copy__().prec == context.prec
assert Context.copy(context).prec == 4
context.clear_flags()
assert not context.flags[Inexact]
context.divide(Decimal(1), Decimal(3))
assert context.flags[Inexact]
assert context.flags.get(Inexact)
assert type(context.flags).get(context.flags, Inexact)
assert len(context.flags.copy()) == len(context.flags)
assert len(context.flags.keys()) == len(context.flags.values())
try:
    context.flags.pop(Inexact)
except ValueError:
    pass
else:
    raise AssertionError('signal deletion succeeded')
assert date.fromtimestamp(1700000000).year == 2023
assert date(2024, 1, 2).fromtimestamp(1700000000) == date.fromtimestamp(1700000000)
assert ForwardRef('int').evaluate() is int
assert ForwardRef.evaluate(ForwardRef('int')) is int
for text in ['a', ''.join(['ab', 'cd']), ''.join(['longer than ', 'fifteen bytes'])]:
    assert str.upper(text) == text.upper()
assert 'abc'.find('', 4) == -1
assert not 'abc'.startswith('', 4)
assert not 'abc'.endswith('', 4)

True
