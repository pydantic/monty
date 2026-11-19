# === Unary slots preserve arbitrary results and use class-only lookup ===
class Unary:
    def __neg__(self):
        return self.result

    def __pos__(self):
        return self.result


unary = Unary()
unary.__neg__ = lambda: 'instance shadow'
unary.__pos__ = None
for result in [None, NotImplemented, [1, 2], unary]:
    unary.result = result
    assert -unary is result
    assert +unary is result


# === Direct and reflected numeric slots ===
operations = [
    ('__add__', '__radd__', lambda a, b: a + b, '+'),
    ('__sub__', '__rsub__', lambda a, b: a - b, '-'),
    ('__mul__', '__rmul__', lambda a, b: a * b, '*'),
    ('__matmul__', '__rmatmul__', lambda a, b: a @ b, '@'),
    ('__truediv__', '__rtruediv__', lambda a, b: a / b, '/'),
    ('__floordiv__', '__rfloordiv__', lambda a, b: a // b, '//'),
    ('__mod__', '__rmod__', lambda a, b: a % b, '%'),
    ('__divmod__', '__rdivmod__', divmod, 'divmod()'),
    ('__pow__', '__rpow__', lambda a, b: a**b, '** or pow()'),
    ('__and__', '__rand__', lambda a, b: a & b, '&'),
    ('__or__', '__ror__', lambda a, b: a | b, '|'),
    ('__xor__', '__rxor__', lambda a, b: a ^ b, '^'),
    ('__lshift__', '__rlshift__', lambda a, b: a << b, '<<'),
    ('__rshift__', '__rrshift__', lambda a, b: a >> b, '>>'),
]
calls = []


def direct(self, other):
    calls.append(('direct', self, other))
    return self.result


def reflected(self, other):
    calls.append(('reflected', self, other))
    return self.result


def raises(self, other):
    raise ValueError('operator failed')


for name, rname, operate, symbol in operations:
    Left = type('Left', (), {name: direct, rname: reflected})
    Right = type('Right', (), {rname: reflected})
    left, right = Left(), Right()
    left.result = ['left']
    right.result = ['right']
    setattr(left, name, None)
    setattr(right, rname, None)

    calls.clear()
    assert operate(left, right) is left.result
    assert calls == [('direct', left, right)]

    left.result = NotImplemented
    calls.clear()
    assert operate(left, right) is right.result
    assert calls == [('direct', left, right), ('reflected', right, left)]

    # Same class: never try __r*__, even if the direct slot is absent.
    second = Left()
    second.result = ['must not be used']
    calls.clear()
    try:
        operate(left, second)
        assert False, 'expected same-type operands to decline'
    except TypeError as exc:
        assert str(exc) == f"unsupported operand type(s) for {symbol}: 'Left' and 'Left'"
    assert calls == [('direct', left, second)]

    calls.clear()
    try:
        operate(right, Right())
        assert False, 'expected reflected-only same-type operands to decline'
    except TypeError as exc:
        assert str(exc) == f"unsupported operand type(s) for {symbol}: 'Right' and 'Right'"
    assert calls == []

    # Distinct classes with identical names are still distinct types.
    Other = type('Left', (), {rname: reflected})
    other = Other()
    other.result = ['other']
    assert operate(left, other) is other.result

    right.result = NotImplemented
    try:
        operate(left, right)
        assert False, 'expected both operands to decline'
    except TypeError as exc:
        assert str(exc) == f"unsupported operand type(s) for {symbol}: 'Left' and 'Right'"

    # None is a successful result, unlike NotImplemented.
    left.result = None
    assert operate(left, right) is None

    # A present but non-callable slot must raise rather than try the other side.
    setattr(Left, name, None)
    calls.clear()
    try:
        operate(left, right)
        assert False, 'expected non-callable slot to fail'
    except TypeError as exc:
        assert str(exc) == "'NoneType' object is not callable"
    assert calls == []

    setattr(Left, name, raises)
    try:
        operate(left, right)
        assert False, 'expected the operator exception to propagate'
    except ValueError as exc:
        assert str(exc) == 'operator failed'

    # Builtin numeric operands must leave user slots their original values.
    setattr(Left, name, direct)
    left.result = ['direct']
    right.result = ['reflected']
    for number in [True, 2, 2.5, 2**80, 2j]:
        calls.clear()
        assert operate(left, number) is left.result
        assert calls == [('direct', left, number)]
        calls.clear()
        assert operate(number, right) is right.result
        assert calls == [('reflected', right, number)]


# === Sequence operands must not bypass user multiplication or reflected addition ===
class SequenceOperand:
    def __mul__(self, other):
        return ('multiply', other)

    def __rmul__(self, other):
        return ('repeat', other)

    def __radd__(self, other):
        return ('concatenate', other)


operand = SequenceOperand()
for sequence in ['abc', b'abc', ''.join(['a', 'bc']), b''.join([b'a', b'bc']), [1], (1,)]:
    assert operand * sequence == ('multiply', sequence)
    assert sequence * operand == ('repeat', sequence)
    assert sequence + operand == ('concatenate', sequence)


# === Power passes a modulus unchanged; None means the two-argument protocol ===
class Power:
    def __pow__(self, *args):
        calls.append(('pow', args))
        return self.result

    def __rpow__(self, *args):
        calls.append(('rpow', args))
        return self.result


class Exponent:
    def __rpow__(self, *args):
        calls.append(('exponent', args))
        return args


power = Power()
power.result = ['power']
exponent = Exponent()
modulus = ['modulus']
for calculate in [lambda: power**True, lambda: pow(power, True), lambda: pow(power, True, None)]:
    calls.clear()
    assert calculate() is power.result
    assert calls == [('pow', (True,))]
    assert calls[0][1][0] is True

calls.clear()
assert pow(power, exponent, modulus) is power.result
assert calls == [('pow', (exponent, modulus))]
power.result = NotImplemented
calls.clear()
assert pow(power, exponent, modulus) == (power, modulus)
assert calls == [('pow', (exponent, modulus)), ('exponent', (power, modulus))]
assert pow(True, exponent, modulus) == (True, modulus)
assert pow(True, exponent, modulus)[0] is True
assert pow(2**80, exponent, modulus) == (2**80, modulus)
assert pow(2j, exponent, modulus) == (2j, modulus)
assert pow(2, exponent, 3.0) == (2, 3.0)

calls.clear()
try:
    pow(2.0, exponent, modulus)
    assert False, 'expected float to reject the modulus before reflected dispatch'
except TypeError as exc:
    assert str(exc) == 'pow() 3rd argument not allowed unless all arguments are integers'
assert calls == []

calls.clear()
try:
    pow(power, Power(), 3)
    assert False, 'expected same-type power to skip reflection'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for ** or pow(): 'Power', 'Power', 'int'"
assert len(calls) == 1
assert calls[0][0] == 'pow'

calls.clear()
try:
    pow(2, 3, power)
    assert False, 'expected modulus not to dispatch user power slots'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for ** or pow(): 'int', 'int', 'Power'"
assert calls == []


# === A bound method stored on the class does not acquire another self ===
class Handler:
    def operate(self, other):
        return (self, other)


handler = Handler()
Bound = type('Bound', (), {'__add__': handler.operate})
assert Bound() + 4 == (handler, 4)

# === Unsupported matrix multiplication now has the ordinary operator error ===
try:
    1 @ 2
    assert False, 'expected unsupported matrix multiplication'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for @: 'int' and 'int'"
