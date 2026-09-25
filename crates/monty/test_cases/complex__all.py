# === Literals and construction ===
assert 1j == complex(0, 1)
assert 2.5j == complex(0, 2.5)
assert 1 + 2j == complex(1, 2)
assert -1j == complex(0, -1)
assert complex() == 0j
assert complex(1) == 1 + 0j
assert complex(1.5) == 1.5 + 0j
assert complex(True, 1) == 1 + 1j
assert complex(imag=2) == 2j
assert complex(real=1) == 1 + 0j
assert complex(1, 2j) == -1 + 0j
assert complex(1j, 1j) == -1 + 1j
assert complex(1 + 2j, 3) == 1 + 5j
assert complex(1 + 2j, 3j) == -2 + 2j
assert complex(1 + 2j, imag=0) == 1 + 2j
assert complex(1 + 2j) == 1 + 2j
assert complex(10**30) == 1e30 + 0j
assert complex(1.5, 10**30) == 1.5 + 1e30j
z = 1 + 2j
assert complex(z) is z
assert complex.from_number(1) == 1 + 0j
assert complex.from_number(1j) == 1j
assert type(1j) is complex
assert isinstance(1j, complex)
assert not isinstance(1, complex)
assert not isinstance(1j, (int, float))
assert complex.__name__ == 'complex'
assert repr(complex) == "<class 'complex'>"

# === Construction from strings ===
assert complex('1+2j') == 1 + 2j
assert complex('(1+2j)') == 1 + 2j
assert complex(' 1+2j ') == 1 + 2j
assert complex('( 1+2j )') == 1 + 2j
assert complex('1+2j\n') == 1 + 2j
assert complex('j') == 1j
assert complex('-j') == -1j
assert complex('+J') == 1j
assert complex('1e3j') == 1000j
assert complex('1e3+j') == 1000 + 1j
assert complex('1+j') == 1 + 1j
assert complex('.5j') == 0.5j
assert complex('5.j') == 5j
assert complex('1_000j') == 1000j
assert complex('1_000') == 1000 + 0j
assert complex('1.5') == 1.5 + 0j
assert complex('(1)') == 1 + 0j
assert complex('-1-2.5J') == -1 - 2.5j
assert complex('infinity') == complex(float('inf'), 0)
assert complex('INFINITYj') == complex(0, float('inf'))
assert complex('inf-infj') == complex(float('inf'), float('-inf'))
assert complex('1e500') == complex(float('inf'), 0)
assert repr(complex('nan+nanj')) == '(nan+nanj)'
assert repr(complex('-nanj')) == 'nanj'

# === Construction errors ===
for bad in ['1 + 2j', '(1', '', '1j+2', '1+2', '1 j', '1e', '0x10', '1+2i', '++1j', '1+-2j', '-', '+', '()', '1.5.2j']:
    try:
        complex(bad)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == 'complex() arg is a malformed string'
for bad in ['1__0j', '_1j', '1_j']:
    try:
        complex(bad)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == f'could not convert string to complex: {bad!r}'
try:
    complex('1', 2)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "complex() argument 'real' must be a real number, not str"
try:
    complex(real='1')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "complex() argument 'real' must be a real number, not str"
try:
    complex(1, '2')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "complex() argument 'imag' must be a real number, not str"
try:
    complex(1, [])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "complex() argument 'imag' must be a real number, not list"
try:
    complex([])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'complex() argument must be a string or a number, not list'
try:
    complex(None)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'complex() argument must be a string or a number, not NoneType'
try:
    complex(b'1')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'complex() argument must be a string or a number, not bytes'
try:
    complex(1, 2, 3)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'complex() takes at most 2 arguments (3 given)'
try:
    complex(10**400)
    assert False, 'expected OverflowError'
except OverflowError as exc:
    assert str(exc) == 'int too large to convert to float'
try:
    complex.from_number('1')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'must be real number, not str'

# === repr and str ===
inf, nan = float('inf'), float('nan')
assert repr(1j) == '1j'
assert repr(-1j) == '(-0-1j)'
assert repr(1 + 2j) == '(1+2j)'
assert repr(1 - 2j) == '(1-2j)'
assert repr(complex(-0.0, 1)) == '(-0+1j)'
assert repr(complex(0.0, -0.0)) == '-0j'
assert repr(complex(-0.0, -0.0)) == '(-0-0j)'
assert repr(complex(1, -0.0)) == '(1-0j)'
assert repr(complex(nan, inf)) == '(nan+infj)'
assert repr(complex(1, nan)) == '(1+nanj)'
assert repr(complex(1e16, 1)) == '(1e+16+1j)'
assert repr(complex(1.5, 2.5e-10)) == '(1.5+2.5e-10j)'
assert repr(complex(1e-5, 1e-4)) == '(1e-05+0.0001j)'
assert repr(complex(0.1 + 0.2, 0)) == '(0.30000000000000004+0j)'
assert repr(complex(2, 0)) == '(2+0j)'
assert repr(-(1 + 0j)) == '(-1-0j)'
assert str(1 + 2j) == '(1+2j)'
assert str(2j) == '2j'
assert f'{1 + 2j}' == '(1+2j)'
assert f'{1 + 2j!r}' == '(1+2j)'
assert '%s' % 1j == '1j'
assert '%r' % 1j == '1j'
assert repr([1j, (1 + 1j)]) == '[1j, (1+1j)]'

# === Attributes and methods ===
assert (1 + 2j).real == 1.0
assert (1 + 2j).imag == 2.0
assert type((1 + 2j).real) is float
assert (1 + 2j).conjugate() == 1 - 2j
try:
    (1 + 2j).conjugate(1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'complex.conjugate() takes no arguments (1 given)'
try:
    (1 + 2j).foo
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'complex' object has no attribute 'foo'"
try:
    (1 + 2j).foo()
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'complex' object has no attribute 'foo'"
try:
    z.real = 2
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == 'readonly attribute'

# === Arithmetic ===
assert 1j * 1j == -1 + 0j
assert (1 + 2j) * (3 + 4j) == -5 + 10j
assert (1 + 2j) / (3 + 4j) == 0.44 + 0.08j
assert 1 / 1j == -1j
assert 1j / 2 == 0.5j
assert (1 + 2j) / 2 == 0.5 + 1j
assert 2 / (1 + 2j) == 0.4 - 0.8j
assert (1 + 2j) + 1 == 2 + 2j
assert 1 + (1 + 2j) == 2 + 2j
assert 1.5 - 1j == 1.5 - 1j
assert 1j - 1.0 == -1 + 1j
assert True - 1j == 1 - 1j
assert 1j - True == -1 + 1j
assert 1j * True == 1j
assert (1 + 2j) / True == 1 + 2j
assert True / 1j == -1j
assert 10**30 + 1j == 1e30 + 1j
assert sum([1j, 2j]) == 3j
assert sum([1, 1j]) == 1 + 1j
assert -(1j) == complex(-0.0, -1)
assert +(1j) == 1j
assert abs(3 + 4j) == 5.0
assert abs(complex(1e308, 1e308)) == 1.4142135623730951e308
assert abs(complex(inf, nan)) == inf
assert abs(complex(nan, 1)) != abs(complex(nan, 1))
assert bool(1j)
assert not bool(0j)
assert not bool(complex(0, -0.0))

# === Mixed-mode rules (a real operand only touches its own part) ===
assert repr(1j * inf) == '(nan+infj)'
assert repr(inf * 1j) == '(nan+infj)'
assert repr(complex(1, 1) * inf) == '(inf+infj)'
assert repr(complex(inf, 1) * 2) == '(inf+2j)'
assert repr(complex(inf, nan) * 1) == '(inf+nanj)'
assert repr(1 - complex(1, 0)) == '-0j'
assert repr(complex(1, 0) - 1) == '0j'
assert repr(1 + complex(1, -0.0)) == '(2-0j)'
assert repr(complex(0, -0.0) + -0.0) == '-0j'
assert repr(complex(0, -0.0) + 0j) == '0j'
assert repr(complex(1, inf) / 2) == '(0.5+infj)'
assert repr(complex(1, inf) / complex(2, 0)) == '(nan+infj)'
assert repr(2 / complex(1, inf)) == '-0j'
assert repr(complex(1, 1) / inf) == '0j'
assert repr(complex(1, 1) / complex(inf, 0)) == '0j'
assert repr(complex(nan, 1) * 2) == '(nan+2j)'
assert repr(complex(inf, 1) * complex(2, 0)) == '(inf+nanj)'
assert repr(complex(inf, 0) * complex(inf, 0)) == '(inf+nanj)'
assert repr((1e308 + 1e308j) / 1e-308) == '(inf+infj)'
assert repr((1e308 + 1e308j) * 1e308) == '(inf+infj)'
assert repr(1e308j * 1e308j) == '(-inf+0j)'
assert repr(complex(0, 0) * complex(inf, 0)) == '(nan+nanj)'
assert repr(0 * complex(inf, 0)) == '(nan+0j)'
assert repr(complex(1, -0.0) * -1) == '(-1+0j)'
assert repr(complex(1, -0.0) / -1) == '(-1+0j)'

# === A real dividend never contributes an imaginary part ===
assert repr(3 / complex(0, -2)) == '1.5j'
assert repr(1.0 / complex(2, 0)) == '(0.5-0j)'
assert repr(1 / complex(-0.0, 1)) == '(-0-1j)'
assert repr(0.0 / complex(1, 1)) == '-0j'
assert repr(-0.0 / complex(1, 1)) == '(-0+0j)'
assert repr(1 / complex(inf, 1)) == '-0j'
assert repr(1 / complex(inf, inf)) == '-0j'
assert repr(1 / complex(nan, inf)) == '-0j'
assert repr(inf / complex(1, 1)) == '(inf-infj)'
assert repr(inf / complex(1e308, 1e308)) == '(nan+nanj)'
assert repr(inf / complex(inf, 1)) == '(nan+nanj)'
assert repr(1e308 / complex(1e-308, 1e-308)) == '(inf-infj)'
assert repr(complex(inf, 0) / complex(1e308, 1e308)) == '(inf-infj)'
assert repr(complex(1, 1) / complex(nan, inf)) == '-0j'
assert 5 / complex(3, 4) == 0.6 - 0.8j
assert True / complex(1, 1) == 0.5 - 0.5j
assert 10**30 / complex(1, 1) == 5e29 - 5e29j

# === Division by zero ===
for expr in [
    lambda: 1j / 0,
    lambda: 1j / 0j,
    lambda: 1 / 0j,
    lambda: 1.5 / 0j,
    lambda: 1j / False,
    lambda: complex(inf, 0) / 0,
]:
    try:
        expr()
        assert False, 'expected ZeroDivisionError'
    except ZeroDivisionError as exc:
        assert str(exc) == 'division by zero'

# === Powers ===
assert 1j**2 == -1 + 0j
assert 1j**100 == 1 + 0j
assert repr(1j**99) == '(-0-1j)'
assert (1 + 2j) ** 2 == -3 + 4j
assert (1 + 2j) ** 2.0 == -3 + 4j
assert (1 + 2j) ** -2 == -0.12 - 0.16j
assert (1 + 1j) ** 100 == -1125899906842624 + 0j
assert (1 + 1j) ** 99 == -562949953421312 + 562949953421312j
assert repr((1 + 1j) ** -100) == '(-8.881784197001252e-16-0j)'
assert repr((1 + 1j) ** 1000) == '(3.273390607896366e+150-2.6303055750020937e+137j)'
assert (2 + 0j) ** 1023 == 8.98846567431158e307 + 0j
assert repr((2 + 0j) ** -1075) == '-0j'
assert 1j**0.5 == 0.7071067811865476 + 0.7071067811865475j
assert 1j**1j == 0.20787957635076193 + 0j
assert 2**1j == 0.7692389013639721 + 0.6389612763136348j
assert 2.5**1j == 0.6087670819712999 + 0.793349002588488j
assert 0j**0 == 1 + 0j
assert 0j**0j == 1 + 0j
assert 0j**2 == 0j
assert 0j**2.5 == 0j
assert 0j**0.0 == 1 + 0j
assert (1 + 2j) ** -0.0 == 1 + 0j
assert 1j**True == 1j
assert True**1j == 1 + 0j
assert 1j**10**30 == 0.5052644514387595 + 0.8629645613304693j
assert (1e-200 + 1e-200j) ** 2 == 0j
assert pow(1j, 2) == -1 + 0j
assert repr(complex(nan, 0) ** 2) == '(nan+nanj)'
for expr in [lambda: 0j**-1, lambda: 0j**1j, lambda: (1e-200 + 1e-200j) ** -2]:
    try:
        expr()
        assert False, 'expected ZeroDivisionError'
    except ZeroDivisionError as exc:
        assert str(exc) == 'zero to a negative or complex power'
for expr in [
    lambda: (2 + 0j) ** 1024,
    lambda: (1e200 + 1e200j) ** 2,
    lambda: 2 ** complex(1024, 0),
    lambda: (1e308 + 0j) ** 1.5,
    lambda: complex(inf, 0) ** 2,
    lambda: complex(inf, 0) ** 0.5,
    lambda: (-1e308) ** 1.5,
]:
    try:
        expr()
        assert False, 'expected OverflowError'
    except OverflowError as exc:
        assert str(exc) == 'complex exponentiation'
try:
    1j ** (10**400)
    assert False, 'expected OverflowError'
except OverflowError as exc:
    assert str(exc) == 'int too large to convert to float'
for expr in [lambda: pow(1j, 2, 3), lambda: pow(1, 2, 1j), lambda: pow(1j, 2, 1.5)]:
    try:
        expr()
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == 'complex modulo'
try:
    pow(1.5, 2, 1j)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'pow() 3rd argument not allowed unless all arguments are integers'
try:
    pow('a', 2, 1j)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for ** or pow(): 'str', 'int', 'complex'"

# === A negative real base with a fractional exponent is complex ===
assert (-8.0) ** (1 / 3) == 1.0000000000000002 + 1.7320508075688772j
assert (-8) ** 0.5 == 1.7319121124709868e-16 + 2.8284271247461903j
assert pow(-8.0, 0.5) == 1.7319121124709868e-16 + 2.8284271247461903j
assert (-1) ** 2.5 == 3.061616997868383e-16 + 1j
assert (-8.0) ** -0.5 == 2.1648901405887335e-17 - 0.3535533905932738j
assert (-1e-308) ** 0.5 == 6.123233995736766e-171 + 1e-154j
assert (-(10**30)) ** 0.5 == 0.06123233995736766 + 1000000000000000j
assert (-1.0) ** inf == 1.0
assert (-2.0) ** inf == inf
assert (-inf) ** 0.5 == inf
assert (-0.0) ** 0.5 == 0.0
assert True**0.5 == 1.0
assert (-8) ** 2 == 64

# === Unsupported operators ===
for message, expr in [
    ("unsupported operand type(s) for //: 'complex' and 'int'", lambda: 1j // 1),
    ("unsupported operand type(s) for //: 'int' and 'complex'", lambda: 1 // 1j),
    ("unsupported operand type(s) for %: 'complex' and 'int'", lambda: 1j % 1),
    ("unsupported operand type(s) for divmod(): 'complex' and 'int'", lambda: divmod(1j, 1)),
    ("unsupported operand type(s) for divmod(): 'int' and 'complex'", lambda: divmod(1, 1j)),
    ("unsupported operand type(s) for &: 'complex' and 'int'", lambda: 1j & 1),
    ("unsupported operand type(s) for <<: 'complex' and 'int'", lambda: 1j << 1),
    ("unsupported operand type(s) for +: 'complex' and 'str'", lambda: 1j + 'a'),
]:
    try:
        expr()
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == message
try:
    ~1j
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "bad operand type for unary ~: 'complex'"
try:
    'a' + 1j
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'can only concatenate str (not "complex") to str'
try:
    10**400 + 1j
    assert False, 'expected OverflowError'
except OverflowError as exc:
    assert str(exc) == 'int too large to convert to float'

# === Equality, ordering and hashing ===
assert 1j == 1j
assert 1j != 2
assert 1 + 0j == 1
assert 1 == 1 + 0j
assert 1.5 + 0j == 1.5
assert True == 1 + 0j
assert complex(-0.0, 0) == 0
assert 10**30 + 0j != 10**30
assert complex(nan, 0) != complex(nan, 0)
assert 1j != 'a'
assert not (1j == 'a')
for message, expr in [
    ("'<' not supported between instances of 'int' and 'complex'", lambda: 1 < 1j),
    ("'<' not supported between instances of 'complex' and 'int'", lambda: 1j < 1),
    ("'<' not supported between instances of 'complex' and 'complex'", lambda: 1j < 1j),
    ("'<=' not supported between instances of 'complex' and 'complex'", lambda: 1j <= 1j),
    ("'>' not supported between instances of 'float' and 'complex'", lambda: 1.5 > 1j),
    ("'<' not supported between instances of 'complex' and 'complex'", lambda: sorted([1j, 2j])),
    ("'>' not supported between instances of 'complex' and 'complex'", lambda: max(1j, 2j)),
]:
    try:
        expr()
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == message
assert hash(1 + 0j) == hash(1)
assert hash(1.5 + 0j) == hash(1.5)
assert hash(1 + 2j) == hash(1 + 2j)
assert hash(1 + 2j) != hash(1 - 2j)
assert {1 + 0j: 1}[1] == 1
assert {1: 1}[1 + 0j] == 1
assert {1j: 'a', 2j: 'b'}[2j] == 'b'
assert 1j in [1j]
assert 1j in {1j}
assert {1j, 1j} == {1j}
assert (1j, 2) == (1j, 2)
assert [1j].index(1j) == 0
assert [1j, 1j, 2j].count(1j) == 2

# === Conversions and builtins that reject complex ===
try:
    int(1j)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "int() argument must be a string, a bytes-like object or a real number, not 'complex'"
try:
    float(1j)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "float() argument must be a string or a real number, not 'complex'"
try:
    round(1j)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "type complex doesn't define __round__ method"
try:
    range(1j)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'complex' object cannot be interpreted as an integer"
try:
    len(1j)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "object of type 'complex' has no len()"
try:
    iter(1j)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'complex' object is not iterable"
try:
    '%f' % 1j
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'must be real number, not complex'
try:
    '%d' % 1j
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == '%d format: a real number is required, not complex'

# === Standard library interplay ===
import copy
import itertools
import json
import math

assert copy.copy(z) is z
assert copy.deepcopy(z) == z
assert list(itertools.islice(itertools.count(1j), 3)) == [1j, 1 + 1j, 2 + 1j]
assert list(itertools.islice(itertools.count(1, 1j), 3)) == [1, 1 + 1j, 1 + 2j]
for fn in [math.sqrt, math.floor, math.isnan]:
    try:
        fn(1j)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == 'must be real number, not complex'
try:
    math.fsum([1j])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'must be real number, not complex'
try:
    json.dumps(1j)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'Object of type complex is not JSON serializable'


# === Iterating a function such as a Julia set ===
def escape_count(z: complex, c: complex) -> int:
    for i in range(50):
        if abs(z) > 2:
            return i
        z = z * z + c
    return 50


assert escape_count(0j, -0.7 + 0.27015j) == 50
assert escape_count(1 + 1j, -0.7 + 0.27015j) == 1
