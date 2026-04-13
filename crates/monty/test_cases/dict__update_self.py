d = {'x': 1}
d.update(d)
assert d == {'x': 1}, 'update self should not change dict'
