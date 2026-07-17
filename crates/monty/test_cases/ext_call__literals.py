# call-external
# External calls in list and dict literals

# External call in list literal
lst = [add_ints(1, 2), add_ints(3, 4)]
assert lst[0] == 3
assert lst[1] == 7

# External call in tuple literal
tup = (add_ints(1, 1), add_ints(2, 2))
assert tup[0] == 2
assert tup[1] == 4

# External call in dict value
d = {'a': add_ints(5, 5), 'b': add_ints(10, 10)}
assert d['a'] == 10
assert d['b'] == 20
