# === Class body vars NOT accessible in methods ===
class ScopeTest:
    x = 10

    def method(self):
        # Class body variable x is NOT in method scope
        # Must access via self.x or ScopeTest.x
        try:
            return x  # NameError
        except NameError:
            return 'no x'

    def correct_method(self):
        return self.x  # Correct: access via self

    def class_access(self):
        return ScopeTest.x  # Also correct: via class name


st = ScopeTest()
assert st.method() == 'no x', 'class body var not in method scope'
assert st.correct_method() == 10, 'class var accessible via self'
assert st.class_access() == 10, 'class var accessible via class name'

# === Class body can reference outer closures ===
# TODO: Requires cell variable support for class bodies (Phase 3)
# def outer():
#     y = 20
#     class Inner:
#         z = y  # This DOES capture y from outer
#         def method(self):
#             return y  # This also captures y from outer
#     return Inner
# InnerCls = outer()
# assert InnerCls.z == 20, 'class body captures outer closure'

# === Class body executes at definition time ===
trace = []


class Tracer:
    trace.append('class body')

    def __init__(self):
        trace.append('init')


assert trace == ['class body'], 'class body runs at definition'
Tracer()
assert trace == ['class body', 'init'], 'init runs at instantiation'
Tracer()
assert trace == ['class body', 'init', 'init'], 'init runs each time'


# === Class variables can reference earlier class variables ===
class VarRef:
    a = 1
    b = a + 2
    c = b * 3


assert VarRef.a == 1, 'first class var'
assert VarRef.b == 3, 'second class var references first'
assert VarRef.c == 9, 'third class var references second'


# === Method cannot reference class variables without self ===
class MethodVarRef:
    x = 10

    def broken(self):
        try:
            return x  # NameError
        except NameError:
            return 'error'

    def correct(self):
        return self.x


mvr = MethodVarRef()
assert mvr.broken() == 'error', 'method cannot access class var without self'
assert mvr.correct() == 10, 'method accesses class var via self'

# === Method captures outer function, not class body ===
# TODO: Requires cell variable support for class bodies (Phase 3)
# def maker(x):
#     class Foo:
#         y = x + 10  # Class body CAN capture outer
#     return Foo

# === Nested class is just a class attribute ===
# TODO: Nested class definitions in class bodies not yet supported
# class Outer:
#     outer_var = 'outer'
#     class Inner:
#         inner_var = 'inner'
# assert Outer.Inner.inner_var == 'inner', 'nested class accessible via outer'
# assert Outer.outer_var == 'outer', 'outer var still accessible'

# === Nested class methods don't see outer class vars ===
# TODO: Requires nested class support
# class Container:
#     container_x = 100
#     class Nested:
#         def method(self):
#             try:
#                 return container_x
#             except NameError:
#                 return 'no access'
# assert Container.Nested().method() == 'no access', 'nested class isolated'


# === Instantiation protocol ===
class Simple:
    def __init__(self, x):
        self.x = x


s = Simple(42)
assert s.x == 42, 'instantiation calls __init__'


# === No __init__ with args raises TypeError ===
class NoInit:
    pass


NoInit()  # OK, no args

try:
    NoInit(1, 2, 3)
    assert False, 'should raise TypeError'
except TypeError as e:
    assert 'takes no arguments' in str(e), 'error message format'
