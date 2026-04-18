# call-external
# Test setattr() builtin function

mut_point = make_mutable_point()

setattr(mut_point, 'x', 100)
assert mut_point.x == 100, 'setattr updated x to 100'

setattr(mut_point, 'y', 200)
assert mut_point.y == 200, 'setattr updated y to 200'

setattr(mut_point, 'z', 300)
assert mut_point.z == 300, 'setattr created new attribute z'

setattr(mut_point, 'name', 'test')
assert mut_point.name == 'test', 'setattr with string value'

setattr(mut_point, 'active', True)
assert mut_point.active == True, 'setattr with bool value'

result = setattr(mut_point, 'x', 999)
assert result == None, 'setattr returns None'

frozen_point = make_point()
try:
    setattr(frozen_point, 'x', 10)
    assert False, 'setattr on frozen dataclass should raise AttributeError'
except AttributeError as e:
    assert str(e) == "cannot assign to field 'x'", str(e)

try:
    setattr()
    assert False, 'setattr() with no args should raise TypeError'
except TypeError as e:
    assert str(e) == 'setattr expected 3 arguments, got 0', str(e)

try:
    setattr(mut_point)
    assert False, 'setattr() with 1 arg should raise TypeError'
except TypeError as e:
    assert str(e) == 'setattr expected 3 arguments, got 1', str(e)

try:
    setattr(mut_point, 'x')
    assert False, 'setattr() with 2 args should raise TypeError'
except TypeError as e:
    assert str(e) == 'setattr expected 3 arguments, got 2', str(e)

try:
    setattr(mut_point, 'x', 10, 'extra')
    assert False, 'setattr() with 4 args should raise TypeError'
except TypeError as e:
    assert str(e) == 'setattr expected 3 arguments, got 4', str(e)

try:
    setattr(mut_point, 123, 'value')
    assert False, 'setattr() with non-string name should raise TypeError'
except TypeError as e:
    assert str(e) == "attribute name must be string, not 'int'", str(e)

try:
    setattr(mut_point, None, 'value')
    assert False, 'setattr() with None name should raise TypeError'
except TypeError as e:
    assert str(e) == "attribute name must be string, not 'NoneType'", str(e)

try:
    setattr(42, 'x', 10)
    assert False, 'setattr on int should raise AttributeError'
except AttributeError as e:
    assert str(e) == "'int' object has no attribute 'x' and no __dict__ for setting new attributes", str(e)

try:
    setattr('hello', 'x', 10)
    assert False, 'setattr on string should raise AttributeError'
except AttributeError as e:
    assert str(e) == "'str' object has no attribute 'x' and no __dict__ for setting new attributes", str(e)
