# call-external
def log(msg):
    print(msg)
    return 1


def inner(x):
    print('Inner called')
    val = add_ints(x, 1)  # External call
    print('Inner resumed')
    return val


def outer():
    print('Outer calling inner')
    # If side effects are duplicated, we'll see "Evaluating arg" twice
    res = inner(log('Evaluating arg'))
    print('Outer returned')
    return res


print('Starting')
res = outer()
print(f'Result: {res}')
assert res == 2
