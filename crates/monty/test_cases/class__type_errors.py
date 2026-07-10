import json


# Error messages involving a user-class instance must name the real class
# (e.g. 'Foo'), not the generic 'object'.
class Foo:
    pass


f = Foo()

# === binary operators ===
try:
    f + 1
    assert False, 'expected + to fail'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for +: 'Foo' and 'int'"
try:
    1 + f
    assert False, 'expected reflected + to fail'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for +: 'int' and 'Foo'"
try:
    f - 1
    assert False, 'expected - to fail'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for -: 'Foo' and 'int'"
try:
    x = Foo()
    x += 1
    assert False, 'expected += to fail'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for +=: 'Foo' and 'int'"
try:
    divmod(f, 1)
    assert False, 'expected divmod to fail'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for divmod(): 'Foo' and 'int'"
try:
    pow(f, 2)
    assert False, 'expected pow to fail'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for ** or pow(): 'Foo' and 'int'"

# === str/list concatenation special form ===
try:
    '' + f
    assert False, 'expected str concat to fail'
except TypeError as exc:
    assert str(exc) == 'can only concatenate str (not "Foo") to str'
try:
    [] + f
    assert False, 'expected list concat to fail'
except TypeError as exc:
    assert str(exc) == 'can only concatenate list (not "Foo") to list'

# === unary operators ===
try:
    -f
    assert False, 'expected unary - to fail'
except TypeError as exc:
    assert str(exc) == "bad operand type for unary -: 'Foo'"
try:
    +f
    assert False, 'expected unary + to fail'
except TypeError as exc:
    assert str(exc) == "bad operand type for unary +: 'Foo'"
try:
    ~f
    assert False, 'expected unary ~ to fail'
except TypeError as exc:
    assert str(exc) == "bad operand type for unary ~: 'Foo'"

# === len / iteration / membership / subscription ===
try:
    len(f)
    assert False, 'expected len to fail'
except TypeError as exc:
    assert str(exc) == "object of type 'Foo' has no len()"
try:
    iter(f)
    assert False, 'expected iter to fail'
except TypeError as exc:
    assert str(exc) == "'Foo' object is not iterable"
try:
    for _ in f:
        pass
    assert False, 'expected for to fail'
except TypeError as exc:
    assert str(exc) == "'Foo' object is not iterable"
try:
    1 in f
    assert False, 'expected membership to fail'
except TypeError as exc:
    assert str(exc) == "argument of type 'Foo' is not a container or iterable"
try:
    a, b = f
    assert False, 'expected unpacking to fail'
except TypeError as exc:
    assert str(exc) == 'cannot unpack non-iterable Foo object'
try:
    f[0]
    assert False, 'expected subscript to fail'
except TypeError as exc:
    assert str(exc) == "'Foo' object is not subscriptable"
try:
    f[0] = 1
    assert False, 'expected subscript assignment to fail'
except TypeError as exc:
    assert str(exc) == "'Foo' object does not support item assignment"

# === calling an instance ===
try:
    f()
    assert False, 'expected call to fail'
except TypeError as exc:
    assert str(exc) == "'Foo' object is not callable"

# === numeric conversions ===
try:
    abs(f)
    assert False, 'expected abs to fail'
except TypeError as exc:
    assert str(exc) == "bad operand type for abs(): 'Foo'"
try:
    int(f)
    assert False, 'expected int() to fail'
except TypeError as exc:
    assert str(exc) == "int() argument must be a string, a bytes-like object or a real number, not 'Foo'"
try:
    float(f)
    assert False, 'expected float() to fail'
except TypeError as exc:
    assert str(exc) == "float() argument must be a string or a real number, not 'Foo'"
try:
    round(f)
    assert False, 'expected round to fail'
except TypeError as exc:
    assert str(exc) == "type Foo doesn't define __round__ method"
try:
    hex(f)
    assert False, 'expected hex to fail'
except TypeError as exc:
    assert str(exc) == "'Foo' object cannot be interpreted as an integer"

# === ordering comparisons ===
try:
    sorted([f, Foo()])
    assert False, 'expected sorted to fail'
except TypeError as exc:
    assert str(exc) == "'<' not supported between instances of 'Foo' and 'Foo'"
# the direct `<` / `>=` operators raise too (not just via sorted)
try:
    f < Foo()
    assert False, 'expected f < Foo() to fail'
except TypeError as exc:
    assert str(exc) == "'<' not supported between instances of 'Foo' and 'Foo'"
try:
    f >= Foo()
    assert False, 'expected f >= Foo() to fail'
except TypeError as exc:
    assert str(exc) == "'>=' not supported between instances of 'Foo' and 'Foo'"

# === string / io / json sinks ===
try:
    ''.join([f])
    assert False, 'expected join to fail'
except TypeError as exc:
    assert str(exc) == 'sequence item 0: expected str instance, Foo found'
try:
    json.dumps(f)
    assert False, 'expected json.dumps to fail'
except TypeError as exc:
    assert str(exc) == 'Object of type Foo is not JSON serializable'
try:
    print(1, 2, sep=f)
    assert False, 'expected print sep to fail'
except TypeError as exc:
    assert str(exc) == 'sep must be None or a string, not Foo'

# === context manager protocol ===
try:
    with f:
        pass
    assert False, 'expected with to fail'
except TypeError as exc:
    assert str(exc) == "'Foo' object does not support the context manager protocol (missed __exit__ method)"


# === two different classes in one message ===
class Bar:
    pass


try:
    f + Bar()
    assert False, 'expected mixed-class + to fail'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for +: 'Foo' and 'Bar'"


# === __repr__ / __str__ must return a str ===
class BadRepr:
    def __repr__(self):
        return 42


try:
    repr(BadRepr())
    assert False, 'expected repr to fail'
except TypeError as exc:
    assert str(exc) == '__repr__ returned non-string (type int)'


class BadStr:
    def __str__(self):
        return 42


try:
    str(BadStr())
    assert False, 'expected str to fail'
except TypeError as exc:
    assert str(exc) == '__str__ returned non-string (type int)'
