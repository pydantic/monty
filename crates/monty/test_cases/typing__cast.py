from typing import assert_never, assert_type, cast

# === cast returns its value untouched, whatever the type argument is ===
assert cast(int, 'x') == 'x', 'cast ignores the type'
items: list[object] = [1]
assert cast('list[int]', items) is items, 'string forward reference'
assert cast(list[int], items) is items, 'generic alias'
assert cast(None, 3) == 3, 'None as the type'
assert cast(typ=int, val=5) == 5, 'keyword arguments'

# === assert_type returns its value ===
ints = [1]
assert assert_type(ints, list[int]) is ints, 'assert_type returns val'
number = int('1')
assert assert_type(number, 'int') == 1, 'assert_type string type'

# === argument errors match CPython ===
try:
    cast()
    assert False, 'cast() should fail'
except TypeError as e:
    assert str(e) == "cast() missing 2 required positional arguments: 'typ' and 'val'", str(e)

try:
    cast(int)
    assert False, 'cast(int) should fail'
except TypeError as e:
    assert str(e) == "cast() missing 1 required positional argument: 'val'", str(e)

try:
    cast(int, 1, 2)
    assert False, 'cast(int, 1, 2) should fail'
except TypeError as e:
    assert str(e) == 'cast() takes 2 positional arguments but 3 were given', str(e)

try:
    assert_type(1)
    assert False, 'assert_type(1) should fail'
except TypeError as e:
    assert str(e) == "assert_type() missing 1 required positional argument: 'typ'", str(e)

try:
    assert_type(1, int, 3)
    assert False, 'assert_type three args should fail'
except TypeError as e:
    assert str(e) == 'assert_type() takes 2 positional arguments but 3 were given', str(e)

# === assert_never always raises, quoting a bounded repr ===
try:
    assert_never(1)
    assert False, 'assert_never should raise'
except AssertionError as e:
    assert str(e) == 'Expected code to be unreachable, but got: 1', str(e)

try:
    assert_never('x' * 200)
    assert False, 'assert_never should raise'
except AssertionError as e:
    assert str(e) == 'Expected code to be unreachable, but got: ' + repr('x' * 200)[:100] + '...', str(e)

try:
    assert_never()
    assert False, 'assert_never() should fail'
except TypeError as e:
    assert str(e) == "assert_never() missing 1 required positional argument: 'arg'", str(e)

try:
    assert_never(1, 2, 3)
    assert False, 'assert_never three args should fail'
except TypeError as e:
    assert str(e) == 'assert_never() takes 1 positional argument but 3 were given', str(e)

# === assert_type and assert_never take their arguments by position only ===
try:
    assert_type(val=1)
    assert False, 'assert_type(val=1) should fail'
except TypeError as e:
    assert str(e) == "assert_type() got some positional-only arguments passed as keyword arguments: 'val'", str(e)

try:
    assert_never(arg=1)
    assert False, 'assert_never(arg=1) should fail'
except TypeError as e:
    assert str(e) == "assert_never() got some positional-only arguments passed as keyword arguments: 'arg'", str(e)
