# call-external
# External function calls inside closures (nested functions with captured variables).


def outer_with_nested():
    x = 10

    def inner():
        return add_ints(x, 5)

    return inner()


assert outer_with_nested() == 15, 'ext call in nested function'
