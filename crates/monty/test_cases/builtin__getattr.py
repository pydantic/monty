# Test getattr() builtin function

s = slice(1, 10, 2)
assert getattr(s, 'start') == 1
assert getattr(s, 'stop') == 10
assert getattr(s, 'step') == 2

assert getattr(s, 'nonexistent', 'default') == 'default'
assert getattr(s, 'nonexistent', None) == None
assert getattr(s, 'nonexistent', 42) == 42

assert getattr(s, 'start', 999) == 1

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
    assert args == ('test error',)

# === Dynamic (heap-allocated) attribute name strings ===
# These test that getattr works with non-interned strings (e.g. from concatenation)
s2 = slice(5, 15, 3)
attr_name = 'sta' + 'rt'
assert getattr(s2, attr_name) == 5

attr_name = 'st' + 'op'
assert getattr(s2, attr_name) == 15

attr_name = 'st' + 'ep'
assert getattr(s2, attr_name) == 3

# Dynamic attribute name with default for missing attribute
attr_name = 'non' + 'existent'
assert getattr(s2, attr_name, 42) == 42

# Dynamic attribute name on exception
try:
    raise TypeError('dynamic test')
except TypeError as e:
    attr_name = 'ar' + 'gs'
    args = getattr(e, attr_name)
    assert args == ('dynamic test',)

# === Dynamic names on builtin types ===
# Builtin attributes dispatch on interned string ids, so a name built at
# runtime has to be resolved back through the interner before dispatch.
import datetime

assert getattr(datetime.timezone, 'ut' + 'c') is datetime.timezone.utc
assert getattr(datetime.time(12, 30), 'ho' + 'ur') == 12
assert getattr(datetime.date(2024, 6, 15), 'mon' + 'th') == 6
assert getattr(datetime.timedelta(hours=1), 'sec' + 'onds') == 3600
# a name the interner has never seen still misses, rather than erroring oddly
assert getattr(datetime.date(2024, 6, 15), 'no' + 'pe', 'MISSING') == 'MISSING'
