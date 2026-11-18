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
except TypeError:
    pass

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
assert SystemExit(1, 2).code == (1, 2)

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
except TypeError:
    pass

try:
    ValueError(a=1)
    assert False, 'should fail on kwarg'
except TypeError:
    pass

try:
    BaseException.__init__(ValueError(), a=1)
    assert False, 'should fail on kwarg'
except TypeError:
    pass

# BaseException.__new__ ignores kwargs and returns new instance
v = BaseException.__new__(ValueError, 'test', a=1)
assert isinstance(v, ValueError)
assert v.args == ('test',)

# BaseException.__init__ reinitializes args
BaseException.__init__(v, 'reinit1', 'reinit2')
assert v.args == ('reinit1', 'reinit2')

try:
    BaseException.__init__(123)
    assert False, 'should fail when self is not an exception'
except TypeError:
    pass

# Custom attribute assignment on exception
e_custom = ValueError('custom_test')
e_custom.extra = 42
assert e_custom.extra == 42
