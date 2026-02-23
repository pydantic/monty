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
except TypeError as e:
    assert str(e) == 'getattr expected at most 3 arguments, got 4', str(e)

try:
    getattr(s, 123)
    assert False, 'getattr() with non-string name should raise TypeError'
except TypeError as e:
    assert str(e) == "attribute name must be string, not 'int'", str(e)

try:
    getattr(s, None)
    assert False, 'getattr() with None name should raise TypeError'
except TypeError as e:
    assert str(e) == "attribute name must be string, not 'NoneType'", str(e)

try:
    raise ValueError('test error')
except ValueError as e:
    args = getattr(e, 'args')
    assert args == ('test error',), 'exception args should be accessible via getattr'

# === Dynamic (heap-allocated) attribute name strings ===
# These test that getattr works with non-interned strings (e.g. from concatenation)
s2 = slice(5, 15, 3)
attr_name = 'sta' + 'rt'
assert getattr(s2, attr_name) == 5, 'getattr with concatenated string should work'

attr_name = 'st' + 'op'
assert getattr(s2, attr_name) == 15, 'getattr with concatenated "stop" should work'

attr_name = 'st' + 'ep'
assert getattr(s2, attr_name) == 3, 'getattr with concatenated "step" should work'

# Dynamic attribute name with default for missing attribute
attr_name = 'non' + 'existent'
assert getattr(s2, attr_name, 42) == 42, 'getattr with dynamic missing attr should return default'

# Dynamic attribute name on exception
try:
    raise TypeError('dynamic test')
except TypeError as e:
    attr_name = 'ar' + 'gs'
    args = getattr(e, attr_name)
    assert args == ('dynamic test',), 'exception args via dynamic string should work'
