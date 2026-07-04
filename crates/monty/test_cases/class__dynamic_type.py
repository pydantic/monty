# === dynamic class creation via 3-arg type() ===
A = type('A', (), {'x': 1})
assert A.__name__ == 'A', 'dynamic class __name__'
assert A.x == 1, 'dynamic class variable'
a = A()
assert type(a) is A, 'type(instance) is the dynamic class'
assert isinstance(a, A), 'isinstance with dynamic class'
assert a.x == 1, 'instance reads class variable'
a.x = 2
assert a.x == 2 and A.x == 1, 'instance attr shadows class var'


# === methods in the namespace dict ===
def _init(self, v):
    self.v = v


def _double(self):
    return self.v * 2


M = type('M', (), {'__init__': _init, 'double': _double, 'halve': lambda self: self.v / 2})
m = M(21)
assert m.v == 21, '__init__ from namespace dict runs'
assert m.double() == 42, 'def method binds self'
assert m.halve() == 10.5, 'lambda method binds self'

# === namespace dict is copied ===
d = {'x': 1}
B = type('B', (), d)
d['x'] = 99
d['y'] = 100
assert B.x == 1, 'later dict mutation does not affect the class'
try:
    B.y
    assert False, 'expected B.y to fail'
except AttributeError as exc:
    assert str(exc) == "type object 'B' has no attribute 'y'", 'copied namespace lacks post-hoc key'

# === __doc__ ===
assert type('N', (), {}).__doc__ is None, '__doc__ defaults to None'
assert type('D', (), {'__doc__': 'hi'}).__doc__ == 'hi', 'explicit __doc__ preserved'

# === identity ===
E = type('E', (), {})
assert E is not type('E', (), {}), 'each call creates a distinct class'

# === indirect and starred calls ===
t = type
Ind = t('Ind', (), {})
assert Ind.__name__ == 'Ind', 'type bound to another name still creates classes'
args = ('S', (), {'v': 7})
S = type(*args)
assert S.__name__ == 'S' and S.v == 7, 'starred 3-arg call'

# === setattr on a dynamic class ===
E.z = 5
assert E.z == 5, 'class attribute assignment on dynamic class'
setattr(E, 'w', 6)
assert E.w == 6, 'setattr on dynamic class'

# === dynamic name computed at runtime ===
name = 'Dyn' + 'Cls'
DC = type(name, (), {})
assert DC.__name__ == 'DynCls', 'runtime-computed class name'
try:
    DC.nope
    assert False, 'expected attribute error'
except AttributeError as exc:
    assert str(exc) == "type object 'DynCls' has no attribute 'nope'", 'errors use the runtime name'

# === 1-arg form still works ===
assert type(1) is int, '1-arg type on int'
assert type(A()) is A, '1-arg type on dynamic instance'

# === arity errors ===
for bad in [lambda: type(), lambda: type('A', ()), lambda: type('A', (), {}, 1)]:
    try:
        bad()
        assert False, 'expected arity error'
    except TypeError as exc:
        assert str(exc) == 'type() takes 1 or 3 arguments', 'arity error message'

# === keyword argument errors ===
try:
    type(1, x=1)
    assert False, 'expected kwargs error'
except TypeError as exc:
    assert str(exc) == 'type() takes no keyword arguments', '1-arg form rejects kwargs'

try:
    type('A', (), {}, x=1)
    assert False, 'expected kwargs error'
except TypeError as exc:
    assert str(exc) == 'A.__init_subclass__() takes no keyword arguments', '3-arg form rejects kwargs'

# === argument type errors (CPython validation order: name, bases, dict) ===
try:
    type(1, (), {})
    assert False, 'expected bad name error'
except TypeError as exc:
    assert str(exc) == 'type.__new__() argument 1 must be str, not int', 'non-str name'

try:
    type(None, (), {})
    assert False, 'expected bad name error'
except TypeError as exc:
    assert str(exc) == 'type.__new__() argument 1 must be str, not None', 'None name uses CPython arg wording'

try:
    type(1, [], {})
    assert False, 'expected bad name error'
except TypeError as exc:
    assert str(exc) == 'type.__new__() argument 1 must be str, not int', 'name checked before bases'

try:
    type('A', [], {})
    assert False, 'expected bad bases error'
except TypeError as exc:
    assert str(exc) == 'type.__new__() argument 2 must be tuple, not list', 'non-tuple bases'

try:
    type('A', (), [])
    assert False, 'expected bad dict error'
except TypeError as exc:
    assert str(exc) == 'type.__new__() argument 3 must be dict, not list', 'non-dict namespace'

try:
    type('A', [], {}, x=1)
    assert False, 'expected bad bases error'
except TypeError as exc:
    assert str(exc) == 'type.__new__() argument 2 must be tuple, not list', 'argument errors beat kwargs error'
