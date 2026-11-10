shared = []
list_source = [shared] * 256
tuple_source = tuple(list_source)
for _ in range(100):
    assert list_source.index(shared, 255) == 255
    assert tuple_source.index(shared, 255) == 255
try:
    list_source.index(None, 256)
    assert False, 'expected ValueError'
except ValueError:
    pass
try:
    tuple_source.index(None, 256)
    assert False, 'expected ValueError'
except ValueError:
    pass
# ref-counts={'shared': 513, 'list_source': 1, 'tuple_source': 1}
