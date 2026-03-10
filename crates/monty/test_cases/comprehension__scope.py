[x for x in range(10)]

try:
    x
    assert False, "Expected NameError for 'x' after comprehension"
except NameError:
    pass
