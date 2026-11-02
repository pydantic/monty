from annotationlib import Format


# === __annotations__ holds evaluated objects, in CPython's order ===
def f(x: int, y: 'Later' = None, *args: str, z: float, **kw: bool) -> list[int]:
    pass


assert list(f.__annotations__) == ['x', 'y', 'args', 'z', 'kw', 'return']
assert f.__annotations__['x'] is int
assert f.__annotations__['y'] == 'Later'
assert f.__annotations__['args'] is str
assert f.__annotations__['z'] is float
assert f.__annotations__['kw'] is bool
assert f.__annotations__['return'] == list[int]

# === __annotate__ is the function computing them (PEP 649) ===
assert f.__annotate__(Format.VALUE) == f.__annotations__
assert f.__annotate__(Format.VALUE_WITH_FAKE_GLOBALS) == f.__annotations__
assert f.__annotate__(1) == f.__annotations__
for fmt in [Format.FORWARDREF, Format.STRING, 5]:
    try:
        f.__annotate__(fmt)
        assert False, 'expected NotImplementedError'
    except NotImplementedError as exc:
        assert str(exc) == ''


# === Unannotated functions and lambdas ===
def g(a, b=1):
    pass


assert g.__annotations__ == {}
assert g.__annotate__ is None
assert (lambda x: x).__annotations__ == {}
assert (lambda x: x).__annotate__ is None


# === Lazy: a forward reference fails on access, not at definition ===
def h(x: Later2) -> Later2:
    pass


try:
    h.__annotations__
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'Later2' is not defined"


class Later2:
    pass


assert h.__annotations__ == {'x': Later2, 'return': Later2}


# === An annotation is an ordinary expression, evaluated each time it is asked for ===
counter = []


def counting():
    counter.append(1)
    return int


def k(a: counting()):
    pass


assert counter == []
assert k.__annotations__ == {'a': int}
assert counter == [1]


# === Methods, via the class and a bound method ===
class K:
    def m(self, a: int) -> str:
        return str(a)

    def plain(self):
        pass


assert K.m.__annotations__ == {'a': int, 'return': str}
assert K().m.__annotations__ == {'a': int, 'return': str}
assert K().m.__annotate__(1) == {'a': int, 'return': str}
assert K.plain.__annotations__ == {}
assert K().plain.__annotate__ is None


# === A function from exec() resolves names in its own globals ===
namespace = {}
exec("def e(a: 'T') -> T:\n    pass", {'T': int}, namespace)
assert namespace['e'].__annotations__ == {'a': 'T', 'return': int}


# === A nested function sees module globals, not the enclosing call's locals ===
def outer():
    def inner(a: Later2):
        pass

    return inner


assert outer().__annotations__ == {'a': Later2}


# === A class built by type() may supply its own __annotate__ ===
def custom_annotate(format):
    return {'a': int}


T = type('T', (), {'__annotate__': custom_annotate})
assert T.__annotations__ == {'a': int}
assert T.__annotations__ is T.__annotations__

# === getattr() and hasattr() see the computed attribute ===
assert getattr(f, '__annotations__')['x'] is int
assert hasattr(f, '__annotations__')
assert getattr(f, '__annotations__', None)['z'] is float
