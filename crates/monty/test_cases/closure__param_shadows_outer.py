# === Parameter shadows outer local (basic) ===
def outer_basic():
    x = 42

    def inner(x):
        return x + 1

    return inner(10)


assert outer_basic() == 11, 'inner param should shadow outer local'


# === Parameter shadows outer local (multiple params) ===
def outer_multi():
    a = 100
    b = 200

    def inner(a, b):
        return a + b

    return inner(1, 2)


assert outer_multi() == 3, 'both params should shadow outer locals'


# === Mixed: one param shadows, one captures ===
def outer_mixed():
    x = 10
    y = 20

    def inner(x):
        return x + y

    return inner(5)


assert outer_mixed() == 25, 'x should be param (5), y should be captured (20)'


# === Parameter shadows with default value ===
def outer_default():
    x = 99

    def inner(x=7):
        return x

    return inner()


assert outer_default() == 7, 'default param should shadow outer local'


# === Deeply nested: param shadows grandparent local ===
def outer_deep():
    x = 1000

    def middle():
        def inner(x):
            return x * 2

        return inner(3)

    return middle()


assert outer_deep() == 6, 'inner param should shadow grandparent local'


# === Parameter used in complex expression ===
def outer_expr():
    scale = 100

    def inner(n, scale):
        return n * scale + 1

    return inner(5, 10)


assert outer_expr() == 51, 'scale param should shadow outer scale'
