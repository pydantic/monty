# call-external
def foo():
    return add_ints(1, 2)


result = foo()
assert result == 3, 'basic ext call in function'
