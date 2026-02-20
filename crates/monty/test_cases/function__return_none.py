# === Bare return statement ===
# Test functions with bare return (no value)


def early_exit():
    return


assert early_exit() is None, 'bare return returns None'


def conditional_early_exit(x):
    if x < 0:
        return
    return x * 2


assert conditional_early_exit(-5) is None, 'conditional early return'
assert conditional_early_exit(5) == 10, 'conditional normal return'


def multiple_bare_returns(x):
    if x == 0:
        return
    if x == 1:
        return
    return x


assert multiple_bare_returns(0) is None, 'first bare return'
assert multiple_bare_returns(1) is None, 'second bare return'
assert multiple_bare_returns(2) == 2, 'fall through to value return'


def nested_bare_return():
    def inner():
        return

    return inner()


assert nested_bare_return() is None, 'nested bare return'
