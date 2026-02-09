# === __str__ and __repr__ ===
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y

    def __str__(self):
        return '(' + str(self.x) + ', ' + str(self.y) + ')'

    def __repr__(self):
        return 'Point(' + repr(self.x) + ', ' + repr(self.y) + ')'


p = Point(3, 4)
assert str(p) == '(3, 4)', '__str__ conversion'
assert repr(p) == 'Point(3, 4)', '__repr__ conversion'


# === __eq__ and __ne__ ===
class Number:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        if not isinstance(other, Number):
            return NotImplemented
        return self.value == other.value


n1 = Number(5)
n2 = Number(5)
n3 = Number(10)
assert n1 == n2, '__eq__ equal'
assert not (n1 == n3), '__eq__ not equal'
assert n1 != n3, '__ne__ derived from __eq__'
assert not (n1 != n2), '__ne__ equal values'


# === __lt__, __le__, __gt__, __ge__ ===
class Comparable:
    def __init__(self, value):
        self.value = value

    def __lt__(self, other):
        if not isinstance(other, Comparable):
            return NotImplemented
        return self.value < other.value

    def __le__(self, other):
        if not isinstance(other, Comparable):
            return NotImplemented
        return self.value <= other.value

    def __gt__(self, other):
        if not isinstance(other, Comparable):
            return NotImplemented
        return self.value > other.value

    def __ge__(self, other):
        if not isinstance(other, Comparable):
            return NotImplemented
        return self.value >= other.value


a = Comparable(1)
b = Comparable(2)
c = Comparable(1)
assert a < b, '__lt__ true'
assert not (b < a), '__lt__ false'
assert a <= b, '__le__ less'
assert a <= c, '__le__ equal'
assert not (b <= a), '__le__ false'
assert b > a, '__gt__ true'
assert not (a > b), '__gt__ false'
assert b >= a, '__ge__ greater'
assert a >= c, '__ge__ equal'
assert not (a >= b), '__ge__ false'


# === __hash__ ===
class Hashable:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        if not isinstance(other, Hashable):
            return NotImplemented
        return self.value == other.value

    def __hash__(self):
        return hash(self.value)


h1 = Hashable(42)
h2 = Hashable(42)
assert hash(h1) == hash(h2), '__hash__ equal objects same hash'
d = {h1: 'found'}
assert d[h2] == 'found', 'hashable as dict key'


# === __bool__ ===
class Truthy:
    def __bool__(self):
        return True


class Falsy:
    def __bool__(self):
        return False


assert bool(Truthy()), '__bool__ True'
assert not bool(Falsy()), '__bool__ False'
if Truthy():
    truthy_branch = True
else:
    truthy_branch = False
assert truthy_branch, '__bool__ in if True'
if Falsy():
    falsy_branch = True
else:
    falsy_branch = False
assert not falsy_branch, '__bool__ in if False'


# === __len__ ===
class SizedThing:
    def __init__(self, n):
        self.n = n

    def __len__(self):
        return self.n


s = SizedThing(5)
assert len(s) == 5, '__len__'
s2 = SizedThing(0)
assert len(s2) == 0, '__len__ zero'


# === __len__ affects truth value when __bool__ absent ===
class LenTruthy:
    def __len__(self):
        return 1


class LenFalsy:
    def __len__(self):
        return 0


assert bool(LenTruthy()), '__len__ nonzero is truthy'
assert not bool(LenFalsy()), '__len__ zero is falsy'


# === __add__ and __radd__ ===
class Vec:
    def __init__(self, x, y):
        self.x = x
        self.y = y

    def __add__(self, other):
        if isinstance(other, Vec):
            return Vec(self.x + other.x, self.y + other.y)
        return NotImplemented

    def __eq__(self, other):
        if not isinstance(other, Vec):
            return NotImplemented
        return self.x == other.x and self.y == other.y


v1 = Vec(1, 2)
v2 = Vec(3, 4)
v3 = v1 + v2
assert v3.x == 4, '__add__ x'
assert v3.y == 6, '__add__ y'


# === __radd__ ===
class MyNum:
    def __init__(self, val):
        self.val = val

    def __radd__(self, other):
        return MyNum(other + self.val)


mn = MyNum(10)
result = 5 + mn
assert result.val == 15, '__radd__ invoked when left operand returns NotImplemented'


# === __iadd__ ===
class Accumulator:
    def __init__(self, val):
        self.val = val

    def __iadd__(self, other):
        self.val = self.val + other
        return self


acc = Accumulator(10)
acc += 5
assert acc.val == 15, '__iadd__'
acc += 3
assert acc.val == 18, '__iadd__ second'


# === __sub__, __rsub__, __isub__ ===
class Num:
    def __init__(self, v):
        self.v = v

    def __sub__(self, other):
        if isinstance(other, Num):
            return Num(self.v - other.v)
        return Num(self.v - other)

    def __rsub__(self, other):
        return Num(other - self.v)

    def __isub__(self, other):
        if isinstance(other, Num):
            self.v = self.v - other.v
        else:
            self.v = self.v - other
        return self

    def __eq__(self, other):
        if isinstance(other, Num):
            return self.v == other.v
        return self.v == other


r = Num(10) - Num(3)
assert r.v == 7, '__sub__'
r2 = 20 - Num(8)
assert r2.v == 12, '__rsub__'
n = Num(10)
n -= 4
assert n.v == 6, '__isub__'


# === __getattribute__ and __getattr__ ===
class AttrHooks:
    def __getattribute__(self, name):
        if name == 'x':
            return 123
        raise AttributeError(name)

    def __getattr__(self, name):
        if name == 'y':
            return 456
        return 'missing:' + name


ah = AttrHooks()
assert ah.x == 123, '__getattribute__ override'
assert ah.y == 456, '__getattr__ fallback'
assert ah.z == 'missing:z', '__getattr__ fallback default'


# === __setattr__ ===
class SetAttrHook:
    count = 0
    last = None

    def __setattr__(self, name, value):
        SetAttrHook.count = SetAttrHook.count + 1
        if name == 'x':
            SetAttrHook.last = value


sa = SetAttrHook()
sa.x = 10
sa.y = 20
assert SetAttrHook.count == 2, '__setattr__ invoked'
assert SetAttrHook.last == 10, '__setattr__ arg value'


# === __delattr__ ===
class DelAttrHook:
    count = 0

    def __init__(self):
        self.x = 1

    def __delattr__(self, name):
        DelAttrHook.count = DelAttrHook.count + 1


da = DelAttrHook()
del da.x
assert DelAttrHook.count == 1, '__delattr__ invoked'


# === __mul__, __rmul__, __imul__ ===
class Factor:
    def __init__(self, v):
        self.v = v

    def __mul__(self, other):
        if isinstance(other, Factor):
            return Factor(self.v * other.v)
        return Factor(self.v * other)

    def __rmul__(self, other):
        return Factor(other * self.v)

    def __imul__(self, other):
        if isinstance(other, Factor):
            self.v = self.v * other.v
        else:
            self.v = self.v * other
        return self


f1 = Factor(3) * Factor(4)
assert f1.v == 12, '__mul__'
f2 = 5 * Factor(6)
assert f2.v == 30, '__rmul__'
f3 = Factor(2)
f3 *= 10
assert f3.v == 20, '__imul__'


# === __truediv__ ===
class Fraction:
    def __init__(self, num, den):
        self.num = num
        self.den = den

    def __truediv__(self, other):
        if isinstance(other, Fraction):
            return Fraction(self.num * other.den, self.den * other.num)
        return Fraction(self.num, self.den * other)


f = Fraction(1, 2) / Fraction(3, 4)
assert f.num == 4, '__truediv__ numerator'
assert f.den == 6, '__truediv__ denominator'


# === __floordiv__ ===
class IntDiv:
    def __init__(self, v):
        self.v = v

    def __floordiv__(self, other):
        return IntDiv(self.v // other.v)


fd = IntDiv(7) // IntDiv(2)
assert fd.v == 3, '__floordiv__'


# === __mod__ ===
class Modular:
    def __init__(self, v):
        self.v = v

    def __mod__(self, other):
        return Modular(self.v % other.v)


m = Modular(10) % Modular(3)
assert m.v == 1, '__mod__'


# === __pow__ ===
class Power:
    def __init__(self, v):
        self.v = v

    def __pow__(self, other):
        return Power(self.v**other.v)


pw = Power(2) ** Power(10)
assert pw.v == 1024, '__pow__'


# === __neg__, __pos__, __abs__ ===
class Signed:
    def __init__(self, v):
        self.v = v

    def __neg__(self):
        return Signed(-self.v)

    def __pos__(self):
        return Signed(abs(self.v))

    def __abs__(self):
        return Signed(abs(self.v))


s = Signed(5)
assert (-s).v == -5, '__neg__'
assert (+Signed(-3)).v == 3, '__pos__'
assert abs(Signed(-7)).v == 7, '__abs__'


# === __invert__ ===
class Bits:
    def __init__(self, v):
        self.v = v

    def __invert__(self):
        return Bits(~self.v)


assert (~Bits(0)).v == -1, '__invert__ 0'
assert (~Bits(5)).v == -6, '__invert__ 5'


# === __int__, __float__, __index__ ===
class MyVal:
    def __init__(self, v):
        self.v = v

    def __int__(self):
        return int(self.v)

    def __float__(self):
        return float(self.v)

    def __index__(self):
        return int(self.v)


mv = MyVal(3.7)
assert int(mv) == 3, '__int__'
assert float(mv) == 3.7, '__float__'

mv2 = MyVal(2)
lst = [10, 20, 30, 40]
assert lst[mv2] == 30, '__index__ for subscript'


# === __contains__ ===
class Range:
    def __init__(self, low, high):
        self.low = low
        self.high = high

    def __contains__(self, item):
        return self.low <= item <= self.high


r = Range(1, 10)
assert 5 in r, '__contains__ True'
assert 1 in r, '__contains__ boundary low'
assert 10 in r, '__contains__ boundary high'
assert 0 not in r, '__contains__ False low'
assert 11 not in r, '__contains__ False high'


# === __getitem__ and __setitem__ ===
class MyList:
    def __init__(self):
        self.data = {}

    def __getitem__(self, key):
        return self.data[key]

    def __setitem__(self, key, value):
        self.data[key] = value

    def __delitem__(self, key):
        del self.data[key]


ml = MyList()
ml[0] = 'hello'
ml[1] = 'world'
assert ml[0] == 'hello', '__getitem__'
assert ml[1] == 'world', '__getitem__ second'
del ml[0]
try:
    ml[0]
    assert False, 'should have raised KeyError'
except KeyError:
    pass


# === __iter__ and __next__ ===
class CountUp:
    def __init__(self, limit):
        self.limit = limit
        self.current = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self.current >= self.limit:
            raise StopIteration
        val = self.current
        self.current += 1
        return val


items = []
for x in CountUp(5):
    items.append(x)
assert items == [0, 1, 2, 3, 4], '__iter__/__next__ in for loop'

# === list() with __iter__ ===
# TODO: list() with instance iterables requires VM-level iteration support
# assert list(CountUp(3)) == [0, 1, 2], 'list() with iterable class'


# === __call__ ===
class Adder:
    def __init__(self, base):
        self.base = base

    def __call__(self, x):
        return self.base + x


add5 = Adder(5)
assert add5(3) == 8, '__call__ basic'
assert add5(10) == 15, '__call__ second'

# callable check
# TODO: callable() builtin needs implementation
# assert callable(add5), 'callable with __call__'
# assert not callable(Point(1, 2)), 'not callable without __call__'


# === __enter__ and __exit__ (context manager) ===
class CM:
    def __init__(self):
        self.entered = False
        self.exited = False
        self.exc_type = None

    def __enter__(self):
        self.entered = True
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.exited = True
        self.exc_type = exc_type
        return False


cm = CM()
with cm as ctx:
    assert ctx is cm, '__enter__ returns self'
    assert cm.entered, '__enter__ called'
    assert not cm.exited, '__exit__ not called yet'
assert cm.exited, '__exit__ called after with block'
assert cm.exc_type is None, '__exit__ no exception'


# === Context manager with exception ===
class SuppressCM:
    def __init__(self):
        self.exc_info = None

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.exc_info = (exc_type, str(exc_val))
        return True  # suppress the exception


scm = SuppressCM()
with scm:
    raise ValueError('test error')
assert scm.exc_info[0] is ValueError, '__exit__ receives exc_type'
assert scm.exc_info[1] == 'test error', '__exit__ receives exc_val'


# === __format__ ===
class Currency:
    def __init__(self, amount):
        self.amount = amount

    def __format__(self, spec):
        if spec == 'dollars':
            return '$' + str(self.amount)
        if spec == 'euros':
            return str(self.amount) + ' EUR'
        return str(self.amount)


c = Currency(42)
# TODO: format() builtin needs implementation
# assert format(c) == '42', '__format__ no spec'
# assert format(c, 'dollars') == '$42', '__format__ dollars'
# assert format(c, 'euros') == '42 EUR', '__format__ euros'


# === __and__, __or__, __xor__ (bitwise) ===
class BitField:
    def __init__(self, v):
        self.v = v

    def __and__(self, other):
        return BitField(self.v & other.v)

    def __or__(self, other):
        return BitField(self.v | other.v)

    def __xor__(self, other):
        return BitField(self.v ^ other.v)

    def __lshift__(self, other):
        return BitField(self.v << other.v)

    def __rshift__(self, other):
        return BitField(self.v >> other.v)


bf1 = BitField(0b1100)
bf2 = BitField(0b1010)
assert (bf1 & bf2).v == 0b1000, '__and__'
assert (bf1 | bf2).v == 0b1110, '__or__'
assert (bf1 ^ bf2).v == 0b0110, '__xor__'
bfl1 = BitField(1)
bfl2 = BitField(3)
result_lshift = bfl1.__lshift__(bfl2)
assert result_lshift.v == 8, '__lshift__'
bfr1 = BitField(16)
bfr2 = BitField(2)
result_rshift = bfr1.__rshift__(bfr2)
assert result_rshift.v == 4, '__rshift__'


# === __matmul__ ===
class Matrix:
    def __init__(self, data):
        self.data = data

    def __matmul__(self, other):
        return 'matmul result'


m1 = Matrix([[1, 2], [3, 4]])
m2 = Matrix([[5, 6], [7, 8]])
assert (m1 @ m2) == 'matmul result', '__matmul__'


# === Reflected operators ===
class RNum:
    def __init__(self, v):
        self.v = v

    def __rsub__(self, other):
        return other - self.v

    def __rmul__(self, other):
        return other * self.v

    def __rtruediv__(self, other):
        return other / self.v

    def __rfloordiv__(self, other):
        return other // self.v

    def __rmod__(self, other):
        return other % self.v

    def __rpow__(self, other):
        return other**self.v


rn = RNum(3)
assert 10 - rn == 7, '__rsub__'
assert 4 * rn == 12, '__rmul__'
assert 9 / rn == 3.0, '__rtruediv__'
assert 10 // rn == 3, '__rfloordiv__'
assert 10 % rn == 1, '__rmod__'
assert 2**rn == 8, '__rpow__'


# === In-place operators ===
class INum:
    def __init__(self, v):
        self.v = v

    def __iadd__(self, other):
        self.v += other
        return self

    def __isub__(self, other):
        self.v -= other
        return self

    def __imul__(self, other):
        self.v *= other
        return self

    def __itruediv__(self, other):
        self.v /= other
        return self

    def __ifloordiv__(self, other):
        self.v //= other
        return self

    def __imod__(self, other):
        self.v %= other
        return self

    def __ipow__(self, other):
        self.v **= other
        return self

    def __iand__(self, other):
        self.v &= other
        return self

    def __ior__(self, other):
        self.v |= other
        return self

    def __ixor__(self, other):
        self.v ^= other
        return self

    def __ilshift__(self, other):
        self.v <<= other
        return self

    def __irshift__(self, other):
        self.v >>= other
        return self


x = INum(10)
x += 5
assert x.v == 15, '__iadd__'
x -= 3
assert x.v == 12, '__isub__'
x *= 2
assert x.v == 24, '__imul__'
x //= 5
assert x.v == 4, '__ifloordiv__'
x **= 3
assert x.v == 64, '__ipow__'
x %= 10
assert x.v == 4, '__imod__'
x = INum(0b1111)
x &= 0b1010
assert x.v == 0b1010, '__iand__'
x |= 0b0101
assert x.v == 0b1111, '__ior__'
x ^= 0b1100
assert x.v == 0b0011, '__ixor__'
x = INum(1)
x <<= 4
assert x.v == 16, '__ilshift__'
x >>= 2
assert x.v == 4, '__irshift__'


# === __eq__ with NotImplemented returns NotImplemented ===
class OnlyEqSelf:
    def __eq__(self, other):
        if isinstance(other, OnlyEqSelf):
            return True
        return NotImplemented


oes = OnlyEqSelf()
assert oes == oes, '__eq__ same type'
assert not (oes == 42), '__eq__ different type returns False'
assert oes != 42, '__ne__ different type returns True'


# === __str__ used by print and str() ===
class Greeting:
    def __str__(self):
        return 'hello world'

    def __repr__(self):
        return 'Greeting()'


g = Greeting()
assert str(g) == 'hello world', 'str() calls __str__'
assert repr(g) == 'Greeting()', 'repr() calls __repr__'


# === __repr__ used when __str__ is absent ===
class OnlyRepr:
    def __repr__(self):
        return 'OnlyRepr()'


assert str(OnlyRepr()) == 'OnlyRepr()', 'str() falls back to __repr__'


# === __bool__ takes precedence over __len__ ===
class BoolOverLen:
    def __bool__(self):
        return False

    def __len__(self):
        return 10


assert not bool(BoolOverLen()), '__bool__ takes precedence over __len__'

# === Dunder lookup on TYPE not instance (Finding #1) ===
# TODO: Requires __dict__ attribute support on instances
# class DunderOnType:
#     def __add__(self, other):
#         return 'class __add__'
# dot = DunderOnType()
# dot.__dict__['__add__'] = lambda self, other: 'instance __add__'
# assert (dot + 1) == 'class __add__', 'dunder looked up on type not instance'
# assert dot.__add__(dot, 1) == 'instance __add__', 'direct access sees instance __dict__'


# === Reflected operator order (Finding #2) ===
class NoReflect:
    def __add__(self, other):
        return 'left wins'


class HasReflect:
    def __radd__(self, other):
        return 'right should not be called'


assert (NoReflect() + HasReflect()) == 'left wins', '__radd__ not called when __add__ succeeds'

# === Reflected operator called when __add__ returns NotImplemented ===
# TODO: Requires NotImplemented singleton support
# class ReturnsNotImpl:
#     def __add__(self, other):
#         return NotImplemented
# class UseReflect:
#     def __radd__(self, other):
#         return 'right called'
# assert (ReturnsNotImpl() + UseReflect()) == 'right called', '__radd__ called when __add__ returns NotImplemented'

# === __eq__ without __hash__ makes unhashable (Finding #3) ===
# TODO: Requires hash() builtin and __eq__-without-__hash__ detection
# class EqOnly:
#     def __eq__(self, other):
#         return True
# caught = False
# try:
#     hash(EqOnly())
# except TypeError as e:
#     caught = True
#     assert 'unhashable type' in str(e), '__eq__ without __hash__ raises TypeError'
# assert caught, '__eq__ without __hash__ is unhashable'

# === __eq__ with explicit __hash__ is hashable ===
# TODO: Requires hash() builtin
# class EqAndHash:
#     def __eq__(self, other):
#         return True
#     def __hash__(self):
#         return 42
# assert hash(EqAndHash()) == 42, '__eq__ with __hash__ is hashable'

# === __getattribute__ called before descriptor (Finding #10) ===
# TODO: Requires __getattribute__/__getattr__ and __dict__ support
# class Intercept:
#     def __getattribute__(self, name):
#         if name == 'secret':
#             return 'intercepted'
#         return super().__getattribute__(name)
# i = Intercept()
# i.__dict__['secret'] = 'original'
# assert i.secret == 'intercepted', '__getattribute__ called before instance dict'

# === __getattr__ only called when __getattribute__ raises AttributeError ===
# TODO: Requires __getattr__ support
# class Fallback:
#     def __getattr__(self, name):
#         return 'fallback'
# f = Fallback()
# assert f.anything == 'fallback', '__getattr__ called for missing attr'
# f.real = 'exists'
# assert f.real == 'exists', '__getattr__ NOT called when attr exists'
