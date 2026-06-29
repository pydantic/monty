# The class body has a real (CPython-like) scope: class variables run
# top-to-bottom in their own namespace, may be arbitrary expressions, and may
# reference earlier class variables. Methods skip the class scope for bare-name
# resolution (a bare name resolves to a global, never a sibling class member).


# === Class var referencing an earlier class var ===
class Stepwise:
    a = 1
    b = a + 1
    c = a + b


assert Stepwise.a == 1, 'first class var'
assert Stepwise.b == 2, 'class var reads an earlier class var'
assert Stepwise.c == 3, 'class var reads two earlier class vars'


# === Class var as an arbitrary expression (call / comprehension) ===
class Computed:
    name = 'abc'.upper()
    squares = [i * i for i in range(4)]
    total = sum(squares)


assert Computed.name == 'ABC', 'class var from a method call'
assert Computed.squares == [0, 1, 4, 9], 'class var from a comprehension'
assert Computed.total == 14, 'class var derived from an earlier class var'


# === Class var evaluation order is top-to-bottom ===
order = []


class Ordered:
    x = order.append('x')
    y = order.append('y')
    z = order.append('z')


assert order == ['x', 'y', 'z'], 'class body runs statements in source order'


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
assert bn.get_helper() == 'global-helper', 'method bare name resolves to global, not class member'
assert bn.get_value() == 'global-value', 'second bare name also resolves to global'
assert BareName.helper == 'member-helper', 'the class member itself is still accessible via the class'
assert BareName.value == 'member-value', 'second class member accessible via the class'


# === Class defined in a function captures enclosing locals (transitive) ===
# `n` flows: enclosing function -> class body (pass-through) -> method.
def make_adder(n):
    class Adder:
        bias = 100  # a class member, not visible to the method by bare name

        def add(self, x):
            return x + n  # captures the enclosing `n`, two scopes up

    return Adder


Adder3 = make_adder(3)
assert Adder3().add(10) == 13, 'method captures enclosing function local through class scope'
assert Adder3.bias == 100, 'class member set in a nested class'
Adder5 = make_adder(5)
assert Adder5().add(10) == 15, 'each class instantiation captures its own enclosing value'
assert Adder3().add(10) == 13, 'independent captures do not interfere'


# === Distinct enclosing-local and class-member names coexist ===
# (The same-name collision is rejected at compile time; distinct names are fine.)
def factory(scale):
    class Widget:
        kind = 'widget'

        def scaled(self, x):
            return x * scale

    return Widget


w = factory(4)()
assert w.scaled(5) == 20, 'method captures enclosing local with a distinct name'
assert factory(4)().kind == 'widget', 'class member with a distinct name is unaffected'


# === Bare-name reference to a class member raises NameError ===
# A method referencing a class member by bare name does NOT find the class
# attribute; with no matching global it is a NameError. (A `try`/`except`
# message assertion is used rather than a TRACEBACK test because CPython adds a
# "Did you mean: 'self.timeout'?" hint to the *display* that Monty does not.)
class Settings:
    timeout = 30

    def describe(self):
        return timeout  # no global `timeout` -> NameError, NOT Settings.timeout


try:
    Settings().describe()
    assert False, 'expected NameError for a bare class-member reference'
except NameError as exc:
    assert str(exc) == "name 'timeout' is not defined", 'bare class-member name is not in scope'
