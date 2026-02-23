# call-external
# Multiple external calls within user-defined functions


def compute_sum():
    a = add_ints(1, 2)
    b = add_ints(3, 4)
    c = add_ints(5, 6)
    return a + b + c


result = compute_sum()
assert result == 21, 'multiple sequential ext calls in func'


def compute_nested():
    return add_ints(add_ints(1, 2), add_ints(3, 4))


result = compute_nested()
assert result == 10, 'nested ext calls in func'


def outer():
    def inner():
        return add_ints(10, 20)

    return inner() + add_ints(1, 2)


result = outer()
assert result == 33, 'ext call in nested func plus outer ext call'
