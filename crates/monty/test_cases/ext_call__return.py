# call-external
# External calls in return statements


def direct_return():
    return add_ints(10, 20)


result = direct_return()
assert result == 30


def return_with_expression():
    return add_ints(1, 2) + add_ints(3, 4)


result = return_with_expression()
assert result == 10


def conditional_return():
    if return_value(True):
        return add_ints(100, 200)
    return add_ints(1, 1)


result = conditional_return()
assert result == 300
