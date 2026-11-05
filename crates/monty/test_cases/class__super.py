# Test cases for super() builtin and Super proxy type.

# --- Single inheritance with super() ---
class Animal:
    def __init__(self, name):
        self.name = name

    def speak(self):
        return f'{self.name} makes a sound'


class Dog(Animal):
    def __init__(self, name, breed):
        super().__init__(name)
        self.breed = breed

    def speak(self):
        parent = super().speak()
        return f'{parent} and barks'


d = Dog('Rex', 'German Shepherd')
print(d.name)
print(d.breed)
print(d.speak())


# --- Diamond multiple inheritance (cooperative MRO) ---
class A:
    def greet(self):
        return ['A']


class B(A):
    def greet(self):
        return ['B'] + super().greet()


class C(A):
    def greet(self):
        return ['C'] + super().greet()


class D(B, C):
    def greet(self):
        return ['D'] + super().greet()


d_obj = D()
print(d_obj.greet())


# --- Two-argument super(Class, self) ---
class Base:
    def foo(self):
        return 'Base.foo'


class Mid(Base):
    def foo(self):
        return 'Mid.foo'


class Sub(Mid):
    def foo(self):
        return 'Sub.foo'

    def mid_foo(self):
        return super().foo()

    def base_foo(self):
        return super(Mid, self).foo()


s = Sub()
print(s.foo())
print(s.mid_foo())
print(s.base_foo())


# --- super(Class, Subclass) type-binding ---
class Shape:
    kind = 'generic shape'


class Polygon(Shape):
    kind = 'polygon'


class Triangle(Polygon):
    kind = 'triangle'


print(super(Triangle, Triangle).kind)
print(super(Polygon, Triangle).kind)


# --- Introspection attributes ---
class X:
    pass


class Y(X):
    def check_super(self):
        s = super()
        print(s.__thisclass__.__name__)
        print(type(s.__self__).__name__)
        print(s.__self_class__.__name__)


y = Y()
y.check_super()

s_unbound = super(Y)
print(s_unbound.__thisclass__.__name__)
print(s_unbound.__self__)
print(s_unbound.__self_class__)

# --- repr tests ---
print(repr(super))
print(repr(super(Y)))
print(repr(super(Y, y)))
print(repr(super(X, Y)))


# --- object.__init__ via super() ---
class Leaf:
    def __init__(self):
        super().__init__()


Leaf()
print('Leaf initialized')

# --- Error conditions ---
try:
    super(1, 2)
except TypeError as e:
    print(e)

try:
    super(X, 'not_an_instance')
except TypeError as e:
    print(e)

try:
    super(Y, X())
except TypeError as e:
    print(e)

try:
    super(1, 2, 3)
except TypeError as e:
    print(e)

try:
    super(x=1)
except TypeError as e:
    print(e)


def free_func():
    super()


try:
    free_func()
except RuntimeError as e:
    print(e)

try:
    super(X, y).nonexistent_attr
except AttributeError as e:
    print(e)


# Follow-up: explicit and zero-argument super() preserve the instance receiver.
class SuperReceiverParent:
    def __init__(self, value):
        self.value = value

    def read(self):
        return self.value


class SuperReceiverChild(SuperReceiverParent):
    def __init__(self, value):
        super().__init__(value + 1)

    def read(self):
        return super().read() * 2

    def explicit_read(self):
        return super(SuperReceiverChild, self).read()


class SuperReceiverGrandchild(SuperReceiverChild):
    def read(self):
        return super().read() + 10


child_rec = SuperReceiverChild(3)
grandchild_rec = SuperReceiverGrandchild(3)
assert child_rec.read() == 8
assert child_rec.explicit_read() == 4
assert grandchild_rec.read() == 18
assert grandchild_rec.explicit_read() == 4
assert super(SuperReceiverGrandchild, grandchild_rec).read() == 8
assert super(SuperReceiverChild, grandchild_rec).read() == 4


# --- Descriptor parity on super() (__init__, __init_subclass__, __new__) ---
class SuperDescCheck:
    def check(self):
        s = super()
        assert hasattr(s, '__init__')
        assert hasattr(s, '__init_subclass__')
        assert hasattr(s, '__new__')
        fn_init = s.__init__
        assert fn_init() is None
        fn_sub = s.__init_subclass__
        assert fn_sub() is None
        fn_new = s.__new__
        other = fn_new(SuperDescCheck)
        assert isinstance(other, SuperDescCheck)


SuperDescCheck().check()


class SuperExcCheck(ValueError):
    def __init__(self, msg):
        fn = super().__init__
        fn(msg, 'extra')

    def allocate(cls):
        fn = super().__new__
        return fn(cls, 'allocated')


sec = SuperExcCheck('boom')
assert sec.args == ('boom', 'extra')
sec2 = SuperExcCheck.allocate(SuperExcCheck)
assert isinstance(sec2, SuperExcCheck) and sec2.args == ('allocated',)


# --- Compiler __class__ cell, nested closures, and dynamic class factories ---
class FactoryBase:
    def val(self):
        return 10


def add_one(cls):
    class Wrapped(cls):
        def val(self):
            return super().val() + 1

        def via_lambda(self):
            return (lambda s: super().val() + 100)(self)

        def via_nested_def(self):
            def inner(s):
                return super().val() + 200

            return inner(self)

        def via_default_arg(self):
            def inner(s=super()):
                return s.val() + 300

            return inner()

        def read_class(self):
            return (lambda: __class__)()

    return Wrapped


DoubleWrapped = add_one(add_one(FactoryBase))
dw = DoubleWrapped()
assert dw.val() == 12
assert dw.via_lambda() == 111
assert dw.via_nested_def() == 211
assert dw.via_default_arg() == 311
assert dw.read_class() is DoubleWrapped


# Reassigning self = None before zero-arg super() produces an unbound super object,
# both when `self` is a plain parameter and when `self` is captured by a closure.
class SelfNoneCheck(FactoryBase):
    def plain_none(self):
        self = None
        return super()

    def captured_none(self):
        keep = lambda: self
        self = None
        assert keep() is None
        return super()


sn_plain = SelfNoneCheck().plain_none()
assert sn_plain.__thisclass__ is SelfNoneCheck and sn_plain.__self__ is None
sn_cap = SelfNoneCheck().captured_none()
assert sn_cap.__thisclass__ is SelfNoneCheck and sn_cap.__self__ is None


# Free function attached outside class body has no __class__ cell.
def detached_method(self):
    return super().val()


class DetachedSub(FactoryBase):
    pass


DetachedSub.m = detached_method
try:
    DetachedSub().m()
    assert False, 'expected RuntimeError'
except RuntimeError as e:
    assert str(e) == 'super(): __class__ cell not found'


# Zero-arg super() in *args-only method raises RuntimeError: super(): no arguments.
class VarArgsOnly(FactoryBase):
    def m(*args):
        return super().val()


try:
    VarArgsOnly().m()
    assert False, 'expected RuntimeError'
except RuntimeError as e:
    assert str(e) == 'super(): no arguments'


# Non-type lexical __class__ captured from an enclosing function raises RuntimeError.
def make_non_type_class_cell(bad_val):
    __class__ = bad_val

    def f(self):
        return super()

    return f


for bad_val, expected_type in [(42, 'int'), (None, 'NoneType'), (FactoryBase(), 'FactoryBase')]:
    try:
        make_non_type_class_cell(bad_val)(1)
        assert False, 'expected RuntimeError'
    except RuntimeError as e:
        assert str(e) == f'super(): __class__ is not a type ({expected_type})'


# __class__ cell is populated before __init_subclass__ runs, and __classcell__ is
# validated and stripped from the resulting class namespace.
init_sub_seen = []


class InitSubHookBase:
    def __init_subclass__(cls):
        inst = cls()
        init_sub_seen.append((inst.my_cls() is cls, inst.sup().__thisclass__ is cls))


class InitSubHookChild(InitSubHookBase):
    def my_cls(self):
        return __class__

    def sup(self):
        return super()


assert init_sub_seen == [(True, True)]

try:
    type('BadCellInt', (), {'__classcell__': 42})
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "__classcell__ must be a nonlocal cell, not <class 'int'>"

try:
    type('BadCellInst', (), {'__classcell__': FactoryBase()})
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "__classcell__ must be a nonlocal cell, not <class 'FactoryBase'>"


# locals() visibility and nested classes / custom __class__ class attribute.
class LocalsAndCustomAttr:
    in_body = '__class__' in locals()
    __class__ = 'custom_attr'

    def no_super(self):
        return '__class__' in locals()

    def with_super(self):
        _ = super
        return '__class__' in locals()

    def get_cls(self):
        return __class__

    def make_inner(self):
        class Inner:
            outer_cls = __class__

            def inner_cls(self):
                return __class__

        return Inner


assert not LocalsAndCustomAttr.in_body
lca = LocalsAndCustomAttr()
assert not lca.no_super()
assert lca.with_super()
assert lca.get_cls() is LocalsAndCustomAttr
InnerCls = lca.make_inner()
assert InnerCls.outer_cls is LocalsAndCustomAttr
assert InnerCls().inner_cls() is InnerCls


# --- Extended super() arity, receiver, descriptor, and MRO edge cases ---
from collections import namedtuple

SupPoint = namedtuple('SupPoint', ['x', 'y'])
sup_pt = SupPoint(1, 2)

for bad_call, expected_msg in [
    (lambda: super([1, 2]), 'super() argument 1 must be a type, not list'),
    (lambda: super([1, 2], [3, 4]), 'super() argument 1 must be a type, not list'),
    (
        lambda: super(X, [1, 2]),
        'super(type, obj): obj (instance of list) is not an instance or subtype of type (X).',
    ),
    (lambda: super([1], [2], [3]), 'super() expected at most 2 arguments, got 3'),
    (
        lambda: super(SupPoint, 1),
        'super(type, obj): obj (instance of int) is not an instance or subtype of type (SupPoint).',
    ),
    (
        lambda: super(int, 'bad'),
        'super(type, obj): obj (instance of str) is not an instance or subtype of type (int).',
    ),
    (
        lambda: super(ValueError, TypeError('x')),
        'super(type, obj): obj (instance of TypeError) is not an instance or subtype of type (ValueError).',
    ),
    (
        lambda: super(type, 42),
        'super(type, obj): obj (instance of int) is not an instance or subtype of type (type).',
    ),
    (
        lambda: super(Y, X),
        'super(type, obj): obj (type X) is not an instance or subtype of type (Y).',
    ),
]:
    try:
        bad_call()
        assert False, f'expected TypeError: {expected_msg}'
    except TypeError as e:
        assert str(e) == expected_msg

# Unbound 2-arg super(X, None) and unbound call_attr error
s_none = super(X, None)
assert s_none.__thisclass__ is X
assert s_none.__self__ is None
assert s_none.__self_class__ is None
assert repr(s_none) == "<super: <class 'X'>, NULL>"

try:
    s_none.missing([1, 2])
    assert False, 'expected AttributeError'
except AttributeError as e:
    assert str(e) == "'super' object has no attribute 'missing'"

# Module top-level zero-arg super()
try:
    super()
    assert False, 'expected RuntimeError'
except RuntimeError as e:
    assert str(e) == 'super(): no arguments'


# Empty __class__ cell when calling a method before class creation finishes
def catch_super_runtime_error(fn):
    try:
        fn(1)
        assert False, 'expected RuntimeError'
    except RuntimeError as e:
        return str(e)


class EarlySuper:
    def m(self):
        return super()

    early_msg = catch_super_runtime_error(m)


assert EarlySuper.early_msg == 'super(): empty __class__ cell'


# Deleted arg[0] via except-as target cleanup
class DeletedSelf(X):
    def check(self):
        try:
            raise ValueError('tmp')
        except ValueError as self:
            pass
        return super()


try:
    DeletedSelf().check()
    assert False, 'expected RuntimeError'
except RuntimeError as e:
    assert str(e) == 'super(): arg[0] deleted'

# NamedTuple, builtin int, super(object, int), and super(type, int) introspection & repr
s_pt = super(SupPoint, sup_pt)
assert repr(s_pt) == "<super: <class 'SupPoint'>, <SupPoint object>>"
assert s_pt.__self_class__ is SupPoint

s_int = super(int, 42)
assert repr(s_int) == "<super: <class 'int'>, <int object>>"
assert s_int.__self_class__ is int
assert s_int.__init__() is None
fn_int_init = s_int.__init__
assert fn_int_init() is None

s_obj_int = super(object, int)
assert repr(s_obj_int) == "<super: <class 'object'>, <int object>>"
assert s_obj_int.__self_class__ is int

s_type_int = super(type, int)
assert repr(s_type_int) == "<super: <class 'type'>, <type object>>"
assert s_type_int.__self_class__ is type

# len, eq, hash, and direct __thisclass__() / __self_class__() calls
try:
    len(super(Y))
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "object of type 'super' has no len()"

s_eq = super(Y, y)
assert s_eq == s_eq
assert s_eq != super(Y, y)
assert isinstance(hash(s_eq), int)
assert isinstance(s_eq.__thisclass__(), Y)
assert isinstance(s_eq.__self_class__(), Y)


# Class-bound super(Base, Sub), callable non-method attributes, and __init_subclass__
class CallableHelper:
    def __init__(self, tag):
        self.tag = tag

    def __call__(self, *args):
        return (self.tag, args)


sup_hook_events = []


class SupRoot:
    cb = CallableHelper('cb')

    def plain(self):
        return ('plain', type(self).__name__)

    def __init_subclass__(cls, **kwargs):
        sup_hook_events.append((cls.__name__, kwargs))


class SupMid(SupRoot):
    pass


class SupLeaf(SupMid):
    pass


sup_hook_events.clear()
fn_plain = super(SupMid, SupLeaf).plain
assert fn_plain(SupLeaf()) == ('plain', 'SupLeaf')
assert super(SupMid, SupLeaf).plain(SupLeaf()) == ('plain', 'SupLeaf')

fn_hook_cls = super(SupMid, SupLeaf).__init_subclass__
fn_hook_cls(from_getattr=1)
super(SupMid, SupLeaf).__init_subclass__(from_call=2)
super(SupMid, SupLeaf()).__init_subclass__(from_inst=3)
assert sup_hook_events == [
    ('SupLeaf', {'from_getattr': 1}),
    ('SupLeaf', {'from_call': 2}),
    ('SupLeaf', {'from_inst': 3}),
]

fn_cb = super(SupLeaf, SupLeaf()).cb
assert fn_cb(10) == ('cb', (10,))
assert super(SupLeaf, SupLeaf()).cb(20) == ('cb', (20,))

assert super(X, Y).__init_subclass__() is None
fn_def_sub = super(X, Y).__init_subclass__
assert fn_def_sub() is None


# Custom __new__ and callable non-method __init__ via super()
class CustomNewRoot:
    def __new__(cls, tag='d'):
        obj = object.__new__(cls)
        obj.tag = tag
        return obj


class CustomNewChild(CustomNewRoot):
    pass


cnc = CustomNewChild('hi')
fn_cn = super(CustomNewChild, cnc).__new__
cnc2 = fn_cn(CustomNewChild, 'via_getattr')
assert isinstance(cnc2, CustomNewChild) and cnc2.tag == 'via_getattr'
cnc3 = super(CustomNewChild, cnc).__new__(CustomNewChild, 'via_call')
assert isinstance(cnc3, CustomNewChild) and cnc3.tag == 'via_call'


class CallableInitRoot:
    __init__ = CallableHelper('init_cb')


class CallableInitChild(CallableInitRoot):
    pass


cic_raw = object.__new__(CallableInitChild)
fn_ci = super(CallableInitChild, cic_raw).__init__
assert fn_ci('a') == ('init_cb', ('a',))
assert super(CallableInitChild, cic_raw).__init__('b') == ('init_cb', ('b',))


# super(ValueError, sub_exc) and super(object, y) empty search bases
class SubValErr(ValueError):
    pass


class SubSubValErr(SubValErr):
    pass


ssve = SubSubValErr('initial')
assert super(ValueError, ssve).__init__('reset') is None
assert ssve.args == ('reset',)
ssve2 = super(SubSubValErr, ssve).__new__(SubSubValErr, 'made2')
assert isinstance(ssve2, SubSubValErr) and ssve2.args == ('made2',)

X.inherited_attr = 123
for bad_attr in [
    lambda: super(object, y).inherited_attr,
    lambda: super(object, y).missing([1, 2]),
    lambda: super(X, y).missing([1, 2]),
]:
    try:
        bad_attr()
        assert False, 'expected AttributeError'
    except AttributeError:
        pass
