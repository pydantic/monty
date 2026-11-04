import math

argument = ['retained argument']
source = b'6g!'[:-1]
for value in [b'6g', source]:
    try:
        bytes.fromhex(value)
    except ValueError:
        pass
    else:
        assert False, 'expected invalid hexadecimal input'
value = None

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
