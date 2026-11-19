child = []
original_tuple = (child, 7)
original_frozenset = frozenset([1, 2, 3])
original_bytes = b'x' * 128

for _ in range(100):
    tuple_alias = tuple(original_tuple)
    frozen_alias = frozenset(original_frozenset)
    bytes_alias = bytes(original_bytes)
    keyword_alias = bytes(source=original_bytes)

assert tuple_alias is original_tuple
assert frozen_alias is original_frozenset
assert bytes_alias is original_bytes
assert keyword_alias is original_bytes
# ref-counts={'child': 2, 'original_tuple': 2, 'tuple_alias': 2, 'original_frozenset': 2, 'frozen_alias': 2, 'original_bytes': 3, 'bytes_alias': 3, 'keyword_alias': 3}
