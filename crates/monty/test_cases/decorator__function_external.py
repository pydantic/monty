# call-external
# Decorators run as ordinary bytecode, so they may suspend on an external call
# — both while the decorator body runs and inside the wrapper it returns.
def scaled(fn):
    factor = add_ints(2, 3)  # suspends while the decorator itself runs

    def wrapper():
        return add_ints(fn(), factor)  # suspends inside the returned wrapper

    return wrapper


@scaled
def five():
    return 5


assert five() == 10


# A decorator *factory* may suspend too, before the decorator is even built.
def offset_by(n):
    shift = add_ints(n, 100)

    def deco(fn):
        return lambda: add_ints(fn(), shift)

    return deco


@offset_by(1)
def seven():
    return 7


assert seven() == 108
