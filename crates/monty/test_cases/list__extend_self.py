# list.extend(self) - CPython checks size of input sequence before copying elements,
# so it can handle the case where the input sequence is the list itself.
# Monty must not panic when the iterable is the list itself.
x = [1, 2, 3]
x.extend(x)
assert x == [1, 2, 3, 1, 2, 3]

for pattern in ([None, True, False], [None, 255, 0]):
    source = pattern * 701
    target = source[:]
    expected = repr(source * 2)
    target.extend(source)
    assert repr(target) == expected
    source.extend(source)
    assert repr(source) == expected
