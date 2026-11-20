# Multi-argument and no-argument built-in exceptions
e = ValueError('a', 'b')
assert e.args == ('a', 'b')
assert str(e) == "('a', 'b')"
assert repr(e) == "ValueError('a', 'b')"

e0 = ValueError()
assert e0.args == ()
assert str(e0) == ''
assert repr(e0) == 'ValueError()'

e1 = ValueError('hello')
assert e1.args == ('hello',)
assert str(e1) == 'hello'
assert repr(e1) == "ValueError('hello')"

# Tuple identity memoization
assert e.args is e.args
assert e0.args is e0.args
assert e1.args is e1.args

# .args reassignment
e = ValueError('x')
e.args = ('single',)
assert str(e) == 'single'
assert repr(e) == "ValueError('single')"

e.args = ()
assert str(e) == ''
assert repr(e) == 'ValueError()'

e.args = [1, 2]
assert e.args == (1, 2)
assert repr(e) == 'ValueError(1, 2)'

try:
    e.args = 5
    assert False, 'should fail on int'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"

# StopIteration .value
assert StopIteration().value is None
assert StopIteration(1).value == 1
assert StopIteration(1, 2).value == 1
s = StopIteration(1)
s.args = (2,)
assert s.value == 1

# SystemExit .code
assert SystemExit().code is None
assert SystemExit(3).code == 3
s_exit = SystemExit(1, 2)
assert s_exit.code == (1, 2)
assert s_exit.code is s_exit.args

# StopIteration and SystemExit __init__
si = StopIteration()
si.__init__(99)
assert si.args == (99,)
assert si.value == 99

se = SystemExit()
se.__init__(100)
assert se.args == (100,)
assert se.code == 100

# OneValue non-string exception args
e_int = ValueError(42)
assert e_int.args == (42,)
assert str(e_int) == '42'
assert repr(e_int) == 'ValueError(42)'

e_list = ValueError([1, 2])
assert e_list.args == ([1, 2],)
assert str(e_list) == '[1, 2]'
assert repr(e_list) == 'ValueError([1, 2])'

k_int = KeyError(42)
assert k_int.args == (42,)
assert str(k_int) == '42'
assert repr(k_int) == 'KeyError(42)'

# Subclasses with defaults and kwargs
mnf = ModuleNotFoundError('mod not found', name='foo', path='bar')
assert mnf.name == 'foo'
assert mnf.path == 'bar'
assert mnf.args == ('mod not found',)

mnf_default = ModuleNotFoundError()
assert mnf_default.name is None
assert mnf_default.path is None

ule = UnboundLocalError('local not bound', name='var')
assert ule.name == 'var'
assert ule.args == ('local not bound',)

ule_default = UnboundLocalError()
assert ule_default.name is None

# Calling instance.__init__
e_reinit = ValueError('first')
e_reinit.__init__('second', 'third')
assert e_reinit.args == ('second', 'third')
assert str(e_reinit) == "('second', 'third')"

# KeyError str formatting
assert str(KeyError()) == ''
assert str(KeyError('k')) == "'k'"
assert str(KeyError('a', 'b')) == "('a', 'b')"

# AttributeError
ea = AttributeError('msg', name='x', obj=1)
assert ea.name == 'x'
assert ea.obj == 1
assert ea.args == ('msg',)
assert AttributeError().name is None
assert AttributeError().obj is None

# NameError
en = NameError('msg', name='x')
assert en.name == 'x'
assert en.args == ('msg',)
assert NameError().name is None

# ImportError
ei = ImportError('msg', name='n', path='p')
assert ei.name == 'n'
assert ei.path == 'p'
assert ei.args == ('msg',)
assert ImportError().name is None
assert ImportError().path is None

# Keyword argument validation
try:
    ImportError('m', bogus=1)
    assert False, 'should fail on unexpected kwarg'
except TypeError as exc:
    assert str(exc) == "ImportError() got an unexpected keyword argument 'bogus'"

try:
    ValueError(a=1)
    assert False, 'should fail on kwarg'
except TypeError as exc:
    assert str(exc) == 'ValueError() takes no keyword arguments'

try:
    BaseException.__init__(ValueError(), a=1)
    assert False, 'should fail on kwarg'
except TypeError as exc:
    assert str(exc) == 'ValueError() takes no keyword arguments'

# BaseException.__new__ error paths
try:
    BaseException.__new__()
    assert False, 'should fail without args'
except TypeError as exc:
    assert str(exc) == 'BaseException.__new__(): not enough arguments'

try:
    BaseException.__new__(123)
    assert False, 'should fail on non-type'
except TypeError as exc:
    assert str(exc) == 'BaseException.__new__(X): X is not a type object (int)'

try:
    BaseException.__new__(int)
    assert False, 'should fail on non-exception type'
except TypeError as exc:
    assert str(exc) == 'BaseException.__new__(int): int is not a subtype of BaseException'

# BaseException.__init__ error paths
try:
    BaseException.__init__()
    assert False, 'should fail without args'
except TypeError as exc:
    assert str(exc) == "descriptor '__init__' of 'BaseException' object needs an argument"

try:
    BaseException.__init__(123)
    assert False, 'should fail when self is not an exception'
except TypeError as exc:
    assert str(exc) == "descriptor '__init__' requires a 'BaseException' object but received a 'int'"

# BaseException.__new__ ignores kwargs and returns new instance
v = BaseException.__new__(ValueError, 'test', a=1)
assert isinstance(v, ValueError)
assert v.args == ('test',)

# BaseException.__init__ reinitializes args
BaseException.__init__(v, 'reinit1', 'reinit2')
assert v.args == ('reinit1', 'reinit2')

# Custom attribute assignment on exception
e_custom = ValueError('custom_test')
e_custom.extra = 42
assert e_custom.extra == 42

# Reinitialization resets specialized fields to None when omitted
s_re = StopIteration('val')
assert s_re.value == 'val'
s_re.__init__()
assert s_re.value is None

se_re = SystemExit(1)
assert se_re.code == 1
se_re.__init__(2)
assert se_re.code == 2
se_re.__init__()
assert se_re.code == 2

ea_re = AttributeError('msg', name='attr_name', obj=123)
assert ea_re.name == 'attr_name'
assert ea_re.obj == 123
ea_re.__init__('new_msg')
assert ea_re.name is None
assert ea_re.obj is None

en_re = NameError('msg', name='var_name')
assert en_re.name == 'var_name'
en_re.__init__('new_msg')
assert en_re.name is None

ei_re = ImportError('msg', name='mod', path='pkg')
assert ei_re.name == 'mod'
assert ei_re.path == 'pkg'
ei_re.__init__('new_msg')
assert ei_re.name is None
assert ei_re.path is None

# Caught exception does not format argument with __str__
class TrapStr:
    def __init__(self):
        self.called = False

    def __str__(self):
        self.called = True
        return 'trap'

trap = TrapStr()
saved_caught = None
try:
    raise ValueError(trap)
except ValueError as caught:
    assert not trap.called
    assert caught.args[0] is trap
    saved_caught = caught

assert str(saved_caught) == 'trap'
assert trap.called

# KeyError str with non-string and bytes
assert str(KeyError(123)) == '123'
assert str(KeyError(b'abc')) == "b'abc'"
