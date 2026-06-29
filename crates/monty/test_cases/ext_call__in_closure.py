# call-external
# External function calls inside closures (nested functions with captured variables).


def outer_with_nested():
    x = 10

    def inner():
        return add_ints(x, 5)

    return inner()


assert outer_with_nested() == 15, 'ext call in nested function'


# An external call (which suspends the frame) inside a closure capturing a
# variable *two* levels up — exercises suspend/resume of frames holding a
# transitively threaded cell.
def outer_two_levels():
    x = 100

    def mid():
        def inner():
            return add_ints(x, 7)

        return inner()

    return mid()


assert outer_two_levels() == 107, 'ext call in a two-level closure'
