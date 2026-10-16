import functools


# === Rebinding a method on the class after it has been called ===
class A:
    def m(self):
        return 'old'


def new_m(self):
    return 'new'


a = A()
results = []
for i in range(3):
    results.append(a.m())
A.m = new_m
for i in range(3):
    results.append(a.m())
assert results == ['old', 'old', 'old', 'new', 'new', 'new']

# setattr() on the class
setattr(A, 'm', lambda self: 'lambda')
assert a.m() == 'lambda'

# === Replacing a method with a data attribute ===
A.m = 5
try:
    a.m()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not callable"

# a non-function class attribute is called unbound
A.m = len
assert a.m([1, 2]) == 2


# === An instance attribute shadows a warm method ===
class B:
    def m(self):
        return 'method'


b = B()
assert b.m() == 'method'
b.m = lambda: 'instance'
assert b.m() == 'instance'
assert B().m() == 'method'


# === A method that rebinds itself while running ===
class C:
    def m(self):
        C.m = C.second
        return 'first'

    def second(self):
        return 'second'


c = C()
assert [c.m(), c.m(), c.m()] == ['first', 'second', 'second']


# === Each class statement makes a new class, even at the same call site ===
def make(n):
    class D:
        def m(self):
            return n

    return D()


assert [make(n).m() for n in range(20)] == list(range(20))

# classes freed between calls must not leak their methods into new ones
values = []
for n in range(20):

    class E:
        def m(self):
            return n * 2

    values.append(E().m())
assert values == [n * 2 for n in range(20)]

# === Classes made with type() ===
F = type('F', (), {'m': lambda self: 'f'})
G = type('G', (), {'m': lambda self: 'g'})
assert [F().m(), G().m(), F().m()] == ['f', 'g', 'f']
F.m = lambda self: 'f2'
assert [F().m(), G().m()] == ['f2', 'g']


# === Method kinds that bind self: closures, defaults and partial ===
def outer():
    k = 10

    def closure(self):
        return k

    return closure


def with_default(self, x=3):
    return x


def plain(self, x, y):
    return (x, y)


class H:
    cl = outer()
    df = with_default
    pt = functools.partial(plain, y=2)


h = H()
for _ in range(2):
    assert h.cl() == 10
    assert h.df() == 3
    assert h.pt(1) == (1, 2)
H.cl = with_default
assert h.cl() == 3


# === Many classes and names at one call site ===
classes = [type('K' + str(i), (), {'m': (lambda i: lambda self: i)(i)}) for i in range(300)]
instances = [cls() for cls in classes]
assert [obj.m() for obj in instances] == list(range(300))
assert [obj.m() for obj in instances] == list(range(300))


# === Missing methods are reported after a class change ===
class M:
    pass


m = M()
try:
    m.go()
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'M' object has no attribute 'go'"
M.go = lambda self: 'go'
assert m.go() == 'go'
