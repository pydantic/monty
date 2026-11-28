# Nested class definitions: classes defined inside class bodies, lexical scoping
# across nested class bodies, sibling class inheritance, enclosing function
# closures, `__class__` cell isolation, decorators, and `@dataclass` nesting.

from dataclasses import dataclass


# === Basic nested class definition and instantiation ===
class Outer:
    outer_var = 10

    class Inner:
        inner_var = 20

        def __init__(self, x):
            self.x = x

        def compute(self):
            return self.x + self.inner_var

    def __init__(self, y):
        self.y = y

    def make_inner(self, offset):
        return self.Inner(self.y + offset)

    def make_inner_via_cls(self, offset):
        return Outer.Inner(self.y + offset)


assert Outer.outer_var == 10
assert Outer.Inner.inner_var == 20
assert Outer.Inner.__name__ == 'Inner'
assert Outer.__name__ == 'Outer'

inner_from_cls = Outer.Inner(5)
assert inner_from_cls.x == 5
assert inner_from_cls.compute() == 25
assert isinstance(inner_from_cls, Outer.Inner)
assert not isinstance(inner_from_cls, Outer)

outer_inst = Outer(100)
inner_from_inst = outer_inst.Inner(7)
assert inner_from_inst.compute() == 27
assert type(inner_from_inst) is Outer.Inner

made1 = outer_inst.make_inner(3)
assert made1.compute() == 123
made2 = outer_inst.make_inner_via_cls(4)
assert made2.compute() == 124


# === Three-level nesting (A.B.C) ===
class A:
    tag = 'A'

    class B:
        tag = 'B'

        class C:
            tag = 'C'

            def __init__(self, v):
                self.v = v

            def describe(self):
                return (self.tag, self.v)


assert A.tag == 'A'
assert A.B.tag == 'B'
assert A.B.C.tag == 'C'
c_inst = A.B.C(42)
assert c_inst.describe() == ('C', 42)
assert isinstance(c_inst, A.B.C)
assert A().B().C(99).describe() == ('C', 99)


# === Sibling nested classes and inheritance ===
class Container:
    base_offset = 50

    class Base:
        kind = 'base'

        def __init__(self, v):
            self.v = v

        def value(self):
            return self.v

    class Sub(Base):
        kind = 'sub'

        def value(self):
            return self.v * 2

    class DeepSub(Sub):
        kind = 'deepsub'


assert issubclass(Container.Sub, Container.Base)
assert issubclass(Container.DeepSub, Container.Base)
assert issubclass(Container.DeepSub, Container.Sub)

b = Container.Base(10)
s = Container.Sub(10)
ds = Container.DeepSub(10)
assert b.value() == 10
assert s.value() == 20
assert ds.value() == 20
assert b.kind == 'base'
assert s.kind == 'sub'
assert ds.kind == 'deepsub'
assert isinstance(s, Container.Base)
assert isinstance(ds, Container.Base)


# === Outer class locals used in inner class bases, keyword args, and decorators ===
def add_label(label):
    def decorator(cls):
        cls.label = label
        return cls

    return decorator


class HeaderScope:
    prefix = 'hdr'
    recorded = []

    class TrackedBase:
        def __init_subclass__(cls, tag='none'):
            cls.recorded_tag = tag

    def make_base(tag_val):
        HeaderScope_tag = tag_val

        class DynamicBase:
            dyn_tag = HeaderScope_tag

        return DynamicBase

    @add_label(prefix + '-decorated')
    class Child(TrackedBase, tag=prefix + '-kw'):
        pass

    class DynChild(make_base('from-outer-fn')):
        pass


assert HeaderScope.Child.recorded_tag == 'hdr-kw'
assert HeaderScope.Child.label == 'hdr-decorated'
assert HeaderScope.DynChild.dyn_tag == 'from-outer-fn'


# === Lexical scope isolation: outer class vars are NOT visible inside inner class bodies or methods ===
try:

    class ScopeLeakBody:
        secret = 123

        class Inner:
            leaked = secret

    assert False, 'expected NameError when inner class body reads outer class var'
except NameError as e:
    assert str(e) == "name 'secret' is not defined"


class ScopeLeakMethod:
    secret_method_var = 456

    class Inner:
        def get_secret(self):
            return secret_method_var


try:
    ScopeLeakMethod.Inner().get_secret()
    assert False, 'expected NameError when inner class method reads outer class var'
except NameError as e:
    assert str(e) == "name 'secret_method_var' is not defined"


# === Outer class variable shadowing a module global: inner class resolves to module global ===
shadowed_val = 'from-global'


class ShadowOuter:
    shadowed_val = 'from-outer-class'
    seen_in_outer = shadowed_val

    class Inner:
        seen_in_inner_body = shadowed_val

        def seen_in_inner_method(self):
            return shadowed_val


assert ShadowOuter.seen_in_outer == 'from-outer-class'
assert ShadowOuter.Inner.seen_in_inner_body == 'from-global'
assert ShadowOuter.Inner().seen_in_inner_method() == 'from-global'


# === Pre-binding header evaluation: `class C(C)` inside a class body reads global `C` ===
class GlobalBase:
    origin = 'global-base'


class PreBindOuter:
    class GlobalBase(GlobalBase):
        origin = 'nested-override'


assert issubclass(PreBindOuter.GlobalBase, GlobalBase)
assert PreBindOuter.GlobalBase.origin == 'nested-override'
assert GlobalBase.origin == 'global-base'


# === Nested classes inside a function closing over function parameters and locals ===
def make_hierarchy(scale, bias):
    local_tag = 'fn-local'

    class Outer:
        outer_scaled = scale * 2

        class Inner:
            factor = scale
            tag = local_tag

            def __init__(self, v):
                self.v = v

            def run(self):
                return self.v * scale + bias

            class Deep:
                deep_bias = bias

                def combine(self, x):
                    return (x * scale, bias, local_tag)

        def outer_run(self, v):
            return self.Inner(v).run()

    return Outer


H1 = make_hierarchy(3, 10)
H2 = make_hierarchy(5, 100)

assert H1.outer_scaled == 6
assert H1.Inner.factor == 3
assert H1.Inner.tag == 'fn-local'
assert H1.Inner(4).run() == 22
assert H1().outer_run(4) == 22
assert H1.Inner.Deep.deep_bias == 10
assert H1.Inner.Deep().combine(7) == (21, 10, 'fn-local')

assert H2.outer_scaled == 10
assert H2.Inner.factor == 5
assert H2.Inner(4).run() == 120
assert H2.Inner.Deep().combine(7) == (35, 100, 'fn-local')


# === Function closure mutated via `nonlocal` inside nested class method ===
def make_counter_classes(start):
    count = start

    class Outer:
        class Inner:
            def bump(self, delta=1):
                nonlocal count
                count += delta
                return count

            def current(self):
                return count

    return Outer


CounterOuter = make_counter_classes(10)
ci = CounterOuter.Inner()
assert ci.current() == 10
assert ci.bump() == 11
assert ci.bump(4) == 15
assert CounterOuter.Inner().current() == 15


# === `__class__` cell isolation across outer and inner classes ===
class DunderClassOuter:
    def outer_who(self):
        return __class__.__name__

    def outer_is_cls(self):
        return __class__ is DunderClassOuter

    class Inner:
        def inner_who(self):
            return __class__.__name__

        def inner_is_cls(self):
            return __class__ is DunderClassOuter.Inner

        class Deep:
            def deep_who(self):
                return __class__.__name__

            def deep_is_cls(self):
                return __class__ is DunderClassOuter.Inner.Deep

    def outer_after_inner(self):
        return __class__ is DunderClassOuter


dco = DunderClassOuter()
assert dco.outer_who() == 'DunderClassOuter'
assert dco.outer_is_cls() is True
assert dco.outer_after_inner() is True

dci = DunderClassOuter.Inner()
assert dci.inner_who() == 'Inner'
assert dci.inner_is_cls() is True

dcd = DunderClassOuter.Inner.Deep()
assert dcd.deep_who() == 'Deep'
assert dcd.deep_is_cls() is True


# === `@property`, `@classmethod`, and `@staticmethod` on nested classes ===
class DescriptorOuter:
    class Inner:
        factor = 3

        def __init__(self, x):
            self._x = x

        @property
        def x(self):
            return self._x * self.factor

        @x.setter
        def x(self, val):
            self._x = val

        @classmethod
        def from_doubled(cls, v):
            return cls(v * 2)

        @staticmethod
        def add(a, b):
            return a + b


di = DescriptorOuter.Inner.from_doubled(5)
assert di.x == 30
di.x = 4
assert di.x == 12
assert DescriptorOuter.Inner.add(8, 9) == 17
assert di.add(8, 9) == 17


# === `@dataclass` nested inside a normal class and inside another `@dataclass` ===
class Service:
    @dataclass
    class Config:
        host: str
        port: int = 8080

    @dataclass(frozen=True)
    class Token:
        key: str


cfg = Service.Config('localhost')
assert cfg.host == 'localhost'
assert cfg.port == 8080
assert cfg == Service.Config('localhost', 8080)

tok = Service.Token('secret')
assert tok.key == 'secret'


@dataclass
class Envelope:
    sender: str

    @dataclass
    class Payload:
        body: str
        size: int

    payload: Payload


env = Envelope('alice', Envelope.Payload('hello', 5))
assert env.sender == 'alice'
assert env.payload.body == 'hello'
assert env.payload.size == 5
assert env == Envelope('alice', Envelope.Payload('hello', 5))


# === Rebinding / overwriting a nested class name in the outer class body ===
class ReboundInner:
    class Item:
        version = 1

    first_item_cls = Item

    class Item:
        version = 2

    second_item_cls = Item
    Item = 'overwritten-by-str'


assert ReboundInner.first_item_cls.version == 1
assert ReboundInner.second_item_cls.version == 2
assert ReboundInner.Item == 'overwritten-by-str'
