"""Stress next() on a heap iterator that never needs file-buffer handling.

Run with Flying Circus's --benchmark option. The iterator is allocated once;
the hot loop exercises the OpenFile tag rejection and checks the final sum.
"""


def heap_iterator_next():
    iterator = iter(range(200_000))
    checksum = 0
    for _ in range(200_000):
        checksum += next(iterator)
    assert checksum == 19_999_900_000
    assert next(iterator, None) is None
    return checksum


print(heap_iterator_next())
