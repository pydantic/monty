"""Stress type() on heap values whose Python type is determined by HeapKind.

Run with Flying Circus's --benchmark option. This isolates type queries;
allocation happens before the hot loop, and the checksum checks every query.
"""


def heap_type_queries():
    values = [
        [1, 2, 3],
        tuple([1, 2, 3]),
        set([1, 2, 3]),
        frozenset([1, 2, 3]),
        'heap-' + str(123),
        'heap-bytes'.encode(),
    ]
    expected = [list, tuple, set, frozenset, str, bytes]
    checksum = 0
    for _ in range(100_000):
        for index in range(6):
            checksum += type(values[index]) is expected[index]
    assert checksum == 600_000
    return checksum


print(heap_type_queries())
