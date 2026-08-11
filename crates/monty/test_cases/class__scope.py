# The class body has a real (CPython-like) scope: class variables run
# top-to-bottom in their own namespace, may be arbitrary expressions, and may
# reference earlier class variables. Methods skip the class scope for bare-name
# resolution (a bare name resolves to a global, never a sibling class member).


# === Class var referencing an earlier class var ===
class Stepwise:
    a = 1
    b = a + 1
    c = a + b


assert Stepwise.a == 1
assert Stepwise.b == 2
assert Stepwise.c == 3


# === Class var as an arbitrary expression (call / comprehension) ===
class Computed:
    name = 'abc'.upper()
    squares = [i * i for i in range(4)]
    total = sum(squares)


assert Computed.name == 'ABC'
assert Computed.squares == [0, 1, 4, 9]
assert Computed.total == 14


# === Class var evaluation order is top-to-bottom ===
order = []


class Ordered:
    x = order.append('x')
    y = order.append('y')
    z = order.append('z')


assert order == ['x', 'y', 'z']


# === A method does NOT see class members by bare name ===
# `helper` and `value` are class members; the method must resolve the bare names
# to module globals, not to the class attributes.
helper = 'global-helper'
value = 'global-value'


class BareName:
    helper = 'member-helper'
    value = 'member-value'

    def get_helper(self):
        return helper  # the module global, not BareName.helper

    def get_value(self):
        return value  # the module global, not BareName.value


bn = BareName()
assert bn.get_helper() == 'global-helper'
assert bn.get_value() == 'global-value'
assert BareName.helper == 'member-helper'
assert BareName.value == 'member-value'


# === Class defined in a function captures enclosing locals (transitive) ===
# `n` flows: enclosing function -> class body (pass-through) -> method.
def make_adder(n):
    class Adder:
        bias = 100  # a class member, not visible to the method by bare name

        def add(self, x):
            return x + n  # captures the enclosing `n`, two scopes up

    return Adder


Adder3 = make_adder(3)
assert Adder3().add(10) == 13
assert Adder3.bias == 100
Adder5 = make_adder(5)
assert Adder5().add(10) == 15
assert Adder3().add(10) == 13


# === Distinct enclosing-local and class-member names coexist ===
# (The same-name collision is rejected at compile time; distinct names are fine.)
def factory(scale):
    class Widget:
        kind = 'widget'

        def scaled(self, x):
            return x * scale

    return Widget


w = factory(4)()
assert w.scaled(5) == 20
assert factory(4)().kind == 'widget'

# The bare-name NameError case (a method referencing a class member by bare name)
# is covered by the traceback test in class__name_error.py.


# === LOAD_NAME semantics: a member read before its binding falls back to the
# global namespace (never to enclosing function locals), like CPython ===

x = 5


class ReadsGlobal:
    x = x + 1


assert ReadsGlobal.x == 6
assert x == 5

fwd = 10


class ForwardRef:
    y = fwd
    fwd = 1


assert ForwardRef.y == 10
assert ForwardRef.fwd == 1
assert fwd == 10

g_name = 'global'


def shadowed():
    g_name = 'func'

    class Inner:
        g_name = g_name

    assert g_name == 'func'
    return Inner.g_name


assert shadowed() == 'global'


# A global created at runtime, mid-class-body, is visible to a later unbound-
# member read (the fallback is late-bound, not a prepare-time snapshot).
def set_dyn():
    global dyn
    dyn = 99


class DynamicGlobal:
    a = set_dyn()
    b = dyn
    dyn = 1


assert DynamicGlobal.b == 99
assert DynamicGlobal.dyn == 1


# Unbound-member reads fall through globals to builtins.
class BuiltinFallback:
    len = len
    n = len('abc')


assert BuiltinFallback.n == 3


# Method parameter defaults evaluate in class scope at their statement's
# position, so the same before/after-binding rule applies.
w_default = 'module'


class DefaultsScope:
    def before(self, a=w_default):
        return a

    w_default = 'member'

    def after(self, a=w_default):
        return a


d = DefaultsScope()
assert d.before() == 'module'
assert d.after() == 'member'

# A member with no global/builtin anywhere raises NameError (not
# UnboundLocalError), matching CPython class bodies.
try:

    class NoBinding:
        z = missing_name + 1
        missing_name = 1

    assert False, 'expected NameError'
except NameError as e:
    assert str(e) == "name 'missing_name' is not defined"
