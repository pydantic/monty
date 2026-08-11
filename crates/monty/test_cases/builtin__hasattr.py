# Test hasattr() builtin function

s = slice(1, 10, 2)

assert hasattr(s, 'start') == True
assert hasattr(s, 'stop') == True
assert hasattr(s, 'step') == True

assert hasattr(s, 'nonexistent') == False
assert hasattr(s, 'foo') == False
assert hasattr(s, 'bar') == False

try:
    raise ValueError('test error')
except ValueError as e:
    assert hasattr(e, 'args') == True
    assert hasattr(e, 'nonexistent') == False

assert hasattr(42, 'start') == False
assert hasattr('hello', 'nonexistent') == False

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
