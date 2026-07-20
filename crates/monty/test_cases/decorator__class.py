# === Class decorator: identity returns the class unchanged ===
def identity(cls):
    return cls


@identity
class Foo:
    x = 1


assert Foo.x == 1
assert Foo().x == 1


# === Class decorator can replace the class with any value ===
def make_marker(cls):
    return 'decorated'


@make_marker
class Bar:
    pass


assert Bar == 'decorated'


# === Decorator factory (decorator with arguments) ===
def tag(label):
    def deco(cls):
        cls.label = label
        return cls

    return deco


@tag('hello')
class Baz:
    pass


assert Baz.label == 'hello'


# === Stacked decorators apply bottom-up (nearest the class first) ===
order = []


def first(cls):
    order.append('first')
    return cls


def second(cls):
    order.append('second')
    return cls


@first
@second
class Multi:
    pass


assert order == ['second', 'first']


# === Registry pattern: decorator records the class and returns it ===
REGISTRY = {}


def register(cls):
    REGISTRY[cls.__name__] = cls
    return cls


@register
class Alpha:
    pass


@register
class Beta:
    pass


assert sorted(REGISTRY) == ['Alpha', 'Beta']
assert REGISTRY['Alpha'] is Alpha


# === Method injection: decorator adds a method usable on instances ===
def add_greet(cls):
    def greet(self):
        return 'hi'

    cls.greet = greet
    return cls


@add_greet
class Greeter:
    pass


assert Greeter().greet() == 'hi'


# === __init__ injection: the codegen pattern @dataclass builds on ===
def auto_init(cls):
    def __init__(self, x):
        self.x = x

    cls.__init__ = __init__
    return cls


@auto_init
class Point:
    pass


assert Point(9).x == 9


# === __repr__ injection dispatches through repr() ===
def add_repr(cls):
    def __repr__(self):
        return 'custom-repr'

    cls.__repr__ = __repr__
    return cls


@add_repr
class Reprable:
    pass


assert repr(Reprable()) == 'custom-repr'


# === Decorator that replaces the class with an instance (singleton) ===
def singleton(cls):
    return cls()


@singleton
class Config:
    v = 3


assert Config.v == 3


# === Decorators are arbitrary expressions, not just names ===
DECOS = {'k': identity}


@DECOS['k']
class Subscripted:
    y = 2


assert Subscripted.y == 2


# === Decorators evaluate in the enclosing scope and capture its locals ===
def outer():
    captured = 'outer-value'

    def deco(cls):
        cls.tag = captured
        return cls

    @deco
    class Inner:
        pass

    return Inner


assert outer().tag == 'outer-value'


# Multi-hop: the decorator itself comes from a grandparent scope.
def grandparent():
    def deco(cls):
        cls.mark = 1
        return cls

    def middle():
        @deco
        class C:
            pass

        return C

    return middle()


assert grandparent().mark == 1


# Regression: a *nested scope inside the decorator expression* that captures an
# enclosing local needs a cell var too.
def lambda_in_decorator_position():
    n = 5

    @(lambda cls: setattr(cls, 'n', n) or cls)
    class C:
        pass

    return C


assert lambda_in_decorator_position().n == 5


def lambda_passed_to_decorator_factory():
    n = 7

    def factory(fn):
        return fn

    @factory(lambda cls: setattr(cls, 'n', n) or cls)
    class C:
        pass

    return C


assert lambda_passed_to_decorator_factory().n == 7


# === A raising decorator unwinds cleanly and execution continues ===
# The raise happens mid-stack, with the outer decorator still on the operand
# stack; looping checks the cleanup repeats without leaking.
applied = []


def record(cls):
    applied.append('record')
    return cls


def boom(cls):
    raise ValueError('boom')


for _ in range(50):
    try:

        @record
        @boom
        @record
        class Unwind:
            x = 1

        assert False, 'expected the decorator to raise'
    except ValueError as exc:
        assert str(exc) == 'boom'


# only the decorator below `boom` runs; the one above it never does
assert applied == ['record'] * 50


# === A walrus in a decorator expression binds in the enclosing scope ===
# Decorators evaluate in the enclosing scope, so `:=` there makes a local of
# that scope — it must not fall through to the module globals.
def passthrough(v):
    def deco(cls):
        return cls

    return deco


shadowed = 'global'


def walrus_binds_locally():
    @passthrough(shadowed := 'local')
    class C:
        pass

    return shadowed


assert walrus_binds_locally() == 'local'
assert shadowed == 'global'


# The binding is a local, so it never escapes to the module namespace.
def walrus_does_not_leak():
    @passthrough(never_global := 'inner')
    class C:
        pass

    return never_global


assert walrus_does_not_leak() == 'inner'
try:
    never_global
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'never_global' is not defined"


# `global` still redirects the walrus to the module scope.
promoted = 'before'


def walrus_with_global():
    global promoted

    @passthrough(promoted := 'after')
    class C:
        pass


walrus_with_global()
assert promoted == 'after'


# A walrus in a decorator can be captured by a nested function (needs a cell).
def walrus_captured_by_closure():
    @passthrough(captured := 11)
    class C:
        pass

    def read():
        return captured

    return read()


assert walrus_captured_by_closure() == 11


# === Decorator expressions evaluate top-down, before the class body ===
# Apply order is bottom-up, but *evaluation* of the decorator expressions
# happens first and in source order, then the body, then the applications.
events = []


def trace(label):
    events.append('eval:' + label)

    def deco(cls):
        events.append('apply:' + label)
        return cls

    return deco


def body_marker():
    events.append('body')
    return 1


@trace('outer')
@trace('inner')
class Traced:
    marker = body_marker()


assert events == ['eval:outer', 'eval:inner', 'body', 'apply:inner', 'apply:outer']


# === Attribute expressions work in decorator position ===
class Holder:
    pass


def attach(cls):
    cls.via = 'attribute'
    return cls


Holder.deco = attach


@Holder.deco
class ViaAttribute:
    pass


assert ViaAttribute.via == 'attribute'


# === A non-callable decorator raises TypeError ===
try:

    @42
    class NotCallable:
        pass

    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not callable"


# === The class name stays unbound when a decorator raises ===
def raiser(cls):
    raise ValueError('nope')


try:

    @raiser
    class NeverBound:
        pass

    assert False, 'expected ValueError'
except ValueError:
    pass

try:
    NeverBound
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'NeverBound' is not defined"


# === Decoration preserves __name__ and __doc__ ===
def identity_deco(cls):
    return cls


@identity_deco
class Documented:
    """A docstring."""


assert Documented.__name__ == 'Documented'
assert Documented.__doc__ == 'A docstring.'
