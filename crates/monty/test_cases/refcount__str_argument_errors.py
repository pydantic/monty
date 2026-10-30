import math

argument = ['retained argument']
source = b'61!'[:-1]
for value in [b'61', source]:
    try:
        bytes.fromhex(value)
    except TypeError:
        pass
    else:
        assert False, 'bytes.fromhex accepts only str in Monty'
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
