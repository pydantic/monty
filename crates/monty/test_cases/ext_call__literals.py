# call-external
# External calls in list and dict literals

# External call in list literal
lst = [add_ints(1, 2), add_ints(3, 4)]
assert lst[0] == 3, 'ext call in list literal [0]'
assert lst[1] == 7, 'ext call in list literal [1]'

# External call in tuple literal
tup = (add_ints(1, 1), add_ints(2, 2))
assert tup[0] == 2, 'ext call in tuple literal [0]'
assert tup[1] == 4, 'ext call in tuple literal [1]'

# External call in dict value
d = {'a': add_ints(5, 5), 'b': add_ints(10, 10)}
assert d['a'] == 10, 'ext call in dict value a'
assert d['b'] == 20, 'ext call in dict value b'
