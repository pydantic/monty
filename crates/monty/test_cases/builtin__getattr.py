# Test getattr() builtin function

s = slice(1, 10, 2)
assert getattr(s, 'start') == 1, 'getattr(slice, "start") should return 1'
assert getattr(s, 'stop') == 10, 'getattr(slice, "stop") should return 10'
assert getattr(s, 'step') == 2, 'getattr(slice, "step") should return 2'

assert getattr(s, 'nonexistent', 'default') == 'default', 'getattr with default should return default'
assert getattr(s, 'nonexistent', None) == None, 'getattr with None default should return None'
assert getattr(s, 'nonexistent', 42) == 42, 'getattr with numeric default should return number'

assert getattr(s, 'start', 999) == 1, 'getattr should return actual value, not default'

try:
    getattr(s, 'nonexistent')
    assert False, 'getattr should raise AttributeError for missing attribute'
except AttributeError:
    pass

try:
    getattr()
    assert False, 'getattr() with no args should raise TypeError'
except TypeError as e:
    assert str(e) == 'getattr expected at least 2 arguments, got 0', str(e)

try:
    getattr(kwarg=1)
    assert False, 'getattr() with keyword arg should raise TypeError'
except TypeError as e:
    assert str(e) == 'getattr() takes no keyword arguments', str(e)

try:
    getattr(s)
    assert False, 'getattr() with 1 arg should raise TypeError'
except TypeError as e:
    assert str(e) == 'getattr expected at least 2 arguments, got 1', str(e)

try:
    getattr(s, 'start', 'default', 'extra')
    assert False, 'getattr() with 4 args should raise TypeError'
except TypeError:
    pass

try:
    getattr(s, 123)
    assert False, 'getattr() with non-string name should raise TypeError'
except TypeError as e:
    assert 'attribute name must be string' in str(e), 'Error message should mention string requirement'

try:
    getattr(s, None)
    assert False, 'getattr() with None name should raise TypeError'
except TypeError:
    pass

try:
    raise ValueError('test error')
except ValueError as e:
    args = getattr(e, 'args')
    assert args == ('test error',), 'exception args should be accessible via getattr'
