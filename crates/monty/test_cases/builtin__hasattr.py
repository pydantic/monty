# Test hasattr() builtin function

s = slice(1, 10, 2)

assert hasattr(s, 'start') == True, 'hasattr should return True for existing attribute'
assert hasattr(s, 'stop') == True, 'hasattr should return True for stop'
assert hasattr(s, 'step') == True, 'hasattr should return True for step'

assert hasattr(s, 'nonexistent') == False, 'hasattr should return False for missing attribute'
assert hasattr(s, 'foo') == False, 'hasattr should return False for foo'
assert hasattr(s, 'bar') == False, 'hasattr should return False for bar'

try:
    raise ValueError('test error')
except ValueError as e:
    assert hasattr(e, 'args') == True, 'exception should have args attribute'
    assert hasattr(e, 'nonexistent') == False, 'exception should not have nonexistent attribute'

assert hasattr(42, 'start') == False, 'int should not have start attribute'
assert hasattr('hello', 'nonexistent') == False, 'str should not have nonexistent attribute'

try:
    hasattr()
    assert False, 'hasattr() with no args should raise TypeError'
except TypeError as e:
    assert str(e) == 'hasattr expected 2 arguments, got 0', str(e)

try:
    hasattr(s)
    assert False, 'hasattr() with 1 arg should raise TypeError'
except TypeError as e:
    assert str(e) == 'hasattr expected 2 arguments, got 1', str(e)

try:
    hasattr(s, 'start', 'extra')
    assert False, 'hasattr() with 3 args should raise TypeError'
except TypeError as e:
    assert str(e) == 'hasattr expected 2 arguments, got 3', str(e)

try:
    hasattr(s, 123)
    assert False, 'hasattr() with non-string name should raise TypeError'
except TypeError as e:
    assert str(e) == "attribute name must be string, not 'int'", str(e)

try:
    hasattr(s, None)
    assert False, 'hasattr() with None name should raise TypeError'
except TypeError as e:
    assert str(e) == "attribute name must be string, not 'NoneType'", str(e)
