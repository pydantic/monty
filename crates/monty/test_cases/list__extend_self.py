# list.extend(self) - CPython checks size of input sequence before copying elements,
# so it can handle the case where the input sequence is the list itself.
# Monty must not panic when the iterable is the list itself.
x = [1, 2, 3]
x.extend(x)
assert x == [1, 2, 3, 1, 2, 3], 'extend self should duplicate elements'
