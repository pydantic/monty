# Calling a non-callable still owns the arguments the call evaluated, so the
# TypeError path has to release them. Regression test: sandboxed code could
# otherwise leak a reference per attempt in a loop.
import json
import math

lst = [1, 2, 3]


def call_non_callable():
    # One object per `py_call` arm that can refuse a call: the heap values
    # forwarded to the trait default, and the ones handled by the fallback.
    exc_value = ValueError('x')
    for obj in [[1], {'a': 1}, {1, 2}, (1, 2), 'ab', b'ab', range(3), math, exc_value]:
        try:
            obj(lst)
            assert False, 'expected TypeError'
        except TypeError:
            pass

    positional_args = [1, 2]
    try:
        type(json)(positional_args)
        assert False, 'expected TypeError'
    except TypeError:
        pass
    assert positional_args == [1, 2]

    keyword_args = [3, 4]
    try:
        type(json)(value=keyword_args)
        assert False, 'expected TypeError'
    except TypeError:
        pass
    assert keyword_args == [3, 4]

    combined_args = [5, 6]
    try:
        type(json)(combined_args, value=combined_args)
        assert False, 'expected TypeError'
    except TypeError:
        pass
    assert combined_args == [5, 6]

    return len(lst)


for _ in range(3):
    assert call_non_callable() == 3
# ref-counts={'lst': 1, 'math': 1, 'json': 1}
