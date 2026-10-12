"""Stress equality comparisons that can reject the other heap value by its tag.

All objects are allocated before the loop. The list comparison must decline
six different heap kinds without loading their payloads.
"""


def mixed_heap_comparisons():
    value = [1, 2, 3]
    others = [tuple(value), set(value), {'x': 1}, range(3), 'heap-' + str(123), 'bytes'.encode()]
    checksum = 0
    for _ in range(100_000):
        for other in others:
            checksum += value != other
    assert checksum == 600_000
    return checksum


print(mixed_heap_comparisons())
