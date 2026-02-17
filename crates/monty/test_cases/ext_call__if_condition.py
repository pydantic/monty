# call-external
# External calls in if conditions


def check_positive():
    if add_ints(1, 2) > 0:
        return 'positive'
    return 'not positive'


result = check_positive()
assert result == 'positive', 'ext call in if condition'


def check_with_else():
    if add_ints(-5, 3) > 0:
        return 'positive'
    else:
        return 'negative or zero'


result = check_with_else()
assert result == 'negative or zero', 'ext call in if condition with else'


def check_elif():
    val = add_ints(5, 5)
    if val > 15:
        return 'big'
    elif val > 5:
        return 'medium'
    else:
        return 'small'


result = check_elif()
assert result == 'medium', 'ext call result used in elif chain'
