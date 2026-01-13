//! Comprehensive test covering all types and functions included in monty's typeshed.
//!
//! This test ensures that all builtin functions and classes from
//! `crates/monty-typeshed/update.py` type-check correctly.

use monty_type_checking::type_check;

/// Test all builtin functions type-check correctly.
#[test]
fn builtin_functions() {
    let code = r"
# abs
x1: int = abs(-5)
x2: float = abs(-3.14)

# all / any
b1: bool = all([True, False])
b2: bool = any([True, False])

# bin / hex / oct
s1: str = bin(42)
s2: str = hex(255)
s3: str = oct(8)

# chr / ord
c: str = chr(65)
o: int = ord('A')

# divmod
dm: tuple[int, int] = divmod(10, 3)

# hash
h: int = hash('hello')

# id
i: int = id(object())

# isinstance
is_int: bool = isinstance(42, int)

# len
length: int = len([1, 2, 3])

# max / min
mx: int = max(1, 2, 3)
mn: int = min(1, 2, 3)

# pow
p1: int = pow(2, 3)
p2: float = pow(2.0, 3.0)

# print (returns None)
print('hello')

# repr
r: str = repr(42)

# round
r1: int = round(3.7)

# sorted
sorted_list: list[int] = sorted([3, 1, 2])

# sum
total: int = sum([1, 2, 3])
";

    let result = type_check(code, None).unwrap();
    assert!(result.is_none(), "Expected no type errors, got: {result:?}");
}

/// Test core types (object, type) type-check correctly.
#[test]
fn core_types() {
    let code = r"
# object
obj: object = object()

# type
t: type[int] = type(42)
";

    let result = type_check(code, None).unwrap();
    assert!(result.is_none(), "Expected no type errors, got: {result:?}");
}

/// Test primitive types (bool, int, float) type-check correctly.
#[test]
fn primitive_types() {
    let code = r"
# bool
b1: bool = True
b2: bool = False
b3: bool = bool(1)
b4: bool = bool('')

# int
i1: int = 42
i2: int = int('42')
i3: int = int(3.14)

# float
f1: float = 3.14
f2: float = float('3.14')
f3: float = float(42)
";

    let result = type_check(code, None).unwrap();
    assert!(result.is_none(), "Expected no type errors, got: {result:?}");
}

/// Test string and bytes types type-check correctly.
#[test]
fn string_bytes_types() {
    let code = r"
# str
s1: str = 'hello'
s2: str = str(42)
s3: str = str(b'hello', 'utf-8')

# bytes
b1: bytes = b'hello'
b2: bytes = bytes('hello', 'utf-8')
b3: bytes = bytes(10)
b4: bytes = bytes([65, 66, 67])
";

    let result = type_check(code, None).unwrap();
    assert!(result.is_none(), "Expected no type errors, got: {result:?}");
}

/// Test container types (list, tuple, dict, set, frozenset, range) type-check correctly.
#[test]
fn container_types() {
    let code = r"
# list
lst1: list[int] = [1, 2, 3]
lst2: list[str] = list('abc')
lst3: list[int] = list(range(10))

# tuple
tup1: tuple[int, str, bool] = (1, 'a', True)
tup2: tuple[int, ...] = tuple([1, 2, 3])

# dict
d1: dict[str, int] = {'a': 1, 'b': 2}
d2: dict[str, int] = dict(a=1, b=2)

# set
st1: set[int] = {1, 2, 3}
st2: set[int] = set([1, 2, 3])

# frozenset
fs1: frozenset[int] = frozenset([1, 2, 3])
fs2: frozenset[str] = frozenset('abc')

# range
rng1: range = range(10)
rng2: range = range(0, 10)
rng3: range = range(0, 10, 2)
";

    let result = type_check(code, None).unwrap();
    assert!(result.is_none(), "Expected no type errors, got: {result:?}");
}

/// Test iterator types (enumerate, reversed, zip) type-check correctly.
#[test]
fn iterator_types() {
    let code = r"
# enumerate
for i, v in enumerate([1, 2, 3]):
    x: int = i
    y: int = v

# reversed
for v in reversed([1, 2, 3]):
    z: int = v

# zip
for a, b in zip([1, 2], ['a', 'b']):
    i: int = a
    s: str = b
";

    let result = type_check(code, None).unwrap();
    assert!(result.is_none(), "Expected no type errors, got: {result:?}");
}

/// Test slice type type-checks correctly.
#[test]
fn slice_type() {
    let code = r"
# slice
s1: slice = slice(10)
s2: slice = slice(0, 10)
s3: slice = slice(0, 10, 2)
";

    let result = type_check(code, None).unwrap();
    assert!(result.is_none(), "Expected no type errors, got: {result:?}");
}

/// Test exception hierarchy type-checks correctly.
#[test]
fn exception_types() {
    let code = r"
# BaseException and Exception
e1: BaseException = BaseException('error')
e2: Exception = Exception('error')

# System exceptions
e3: SystemExit = SystemExit(1)
e4: KeyboardInterrupt = KeyboardInterrupt()

# Arithmetic exceptions
e5: ArithmeticError = ArithmeticError('error')
e6: OverflowError = OverflowError('error')
e7: ZeroDivisionError = ZeroDivisionError('error')

# Lookup exceptions
e8: LookupError = LookupError('error')
e9: IndexError = IndexError('error')
e10: KeyError = KeyError('key')

# Runtime exceptions
e11: RuntimeError = RuntimeError('error')
e12: NotImplementedError = NotImplementedError('error')
e13: RecursionError = RecursionError('error')

# Other exceptions
e14: AttributeError = AttributeError('error')
e15: AssertionError = AssertionError('error')
e16: MemoryError = MemoryError('error')
e17: NameError = NameError('error')
e18: SyntaxError = SyntaxError('error')
e19: TimeoutError = TimeoutError('error')
e20: TypeError = TypeError('error')
e21: ValueError = ValueError('error')
e22: StopIteration = StopIteration()
";

    let result = type_check(code, None).unwrap();
    assert!(result.is_none(), "Expected no type errors, got: {result:?}");
}

/// Test exception inheritance relationships.
#[test]
fn exception_inheritance() {
    let code = r"
# All exceptions inherit from BaseException
def handle_base(e: BaseException) -> None:
    pass

handle_base(Exception('error'))
handle_base(ValueError('error'))
handle_base(KeyError('key'))
handle_base(ZeroDivisionError('error'))

# Most exceptions inherit from Exception
def handle_exception(e: Exception) -> None:
    pass

handle_exception(ValueError('error'))
handle_exception(TypeError('error'))
handle_exception(RuntimeError('error'))
";

    let result = type_check(code, None).unwrap();
    assert!(result.is_none(), "Expected no type errors, got: {result:?}");
}

/// Test try/except with exception types.
#[test]
fn try_except_exceptions() {
    let code = r"
try:
    x = 1 / 0
except ZeroDivisionError as e:
    msg: str = str(e)

try:
    d: dict[str, int] = {}
    v = d['missing']
except KeyError as e:
    pass

try:
    lst: list[int] = []
    v = lst[0]
except IndexError:
    pass
";

    let result = type_check(code, None).unwrap();
    assert!(result.is_none(), "Expected no type errors, got: {result:?}");
}

/// Test raising exceptions.
#[test]
fn raise_exceptions() {
    let code = r"
def may_fail(x: int) -> int:
    if x < 0:
        raise ValueError('x must be non-negative')
    if x == 0:
        raise ZeroDivisionError('x cannot be zero')
    return 100 // x

def not_implemented() -> None:
    raise NotImplementedError('subclass must implement')
";

    let result = type_check(code, None).unwrap();
    assert!(result.is_none(), "Expected no type errors, got: {result:?}");
}
