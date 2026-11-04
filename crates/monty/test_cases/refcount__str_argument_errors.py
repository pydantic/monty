import math

argument = ['retained argument']
source = b'!1x'[:-1]
for value in [b'!1', source]:
    try:
        bytes.fromhex(value)
    except ValueError:
        pass
    else:
        assert False, 'invalid hexadecimal input must fail'
value = None
try:
    bytes.fromhex(argument)
except TypeError:
    pass
else:
    assert False, 'fromhex must reject non-buffer arguments'

alias = list[int]
for operation in [
    lambda: math.missing_method(argument),
    lambda: math.missing_method_longer_than_short(argument),
    lambda: alias.missing_method(argument),
    lambda: alias.missing_method_longer_than_short(argument),
]:
    try:
        operation()
    except AttributeError:
        pass
    else:
        assert False, 'expected an unknown-method error'
operation = None

# ref-counts={'argument': 1, 'source': 1, 'math': 1, 'alias': 1}
