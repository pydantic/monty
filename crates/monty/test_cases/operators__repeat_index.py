from collections import deque, namedtuple

Pair = namedtuple('Pair', ['a', 'b'])
sequences = [[1, 2], deque([1, 2]), (1, 2), Pair(1, 2), 'ab', b'ab']
# Runtime construction keeps these long buffers on the heap.
sequences.extend([''.join(['ab'] * 32), b''.join([b'ab'] * 32)])
events = []


class Count:
    def __init__(self, value=2):
        self.value = value

    def __index__(self):
        events.append('index')
        return self.value


class Reflected:
    def __rmul__(self, other):
        events.append('rmul')
        return 'reflected'

    def __index__(self):
        raise AssertionError('reflected multiplication must run first')


class Declined:
    def __rmul__(self, other):
        events.append('rmul')
        return NotImplemented

    def __index__(self):
        events.append('index')
        return 2


class Unsupported:
    def __rmul__(self, other):
        events.append('rmul')
        return NotImplemented


for sequence in sequences:
    expected = sequence * 2
    events.clear()
    assert sequence * Count() == expected
    assert events == ['index']
    events.clear()
    assert Count() * sequence == expected
    assert events == ['index']
    events.clear()
    assert sequence * Reflected() == 'reflected'
    assert events == ['rmul']
    events.clear()
    assert sequence * Declined() == expected
    assert events == ['rmul', 'index']
    events.clear()
    try:
        sequence * Unsupported()
    except TypeError:
        pass
    else:
        assert False
    assert events == ['rmul']


for make in [list, deque]:
    sequence = make([1])

    class MutatingCount:
        def __index__(self):
            sequence.append(2)
            return 2

    assert list(sequence * MutatingCount()) == [1, 2, 1, 2]
    assert list(sequence) == [1, 2]


class InvalidCount:
    def __index__(self):
        return '2'


class RaisingCount:
    def __index__(self):
        raise ValueError('count failed')


# Parsing at runtime also exercises cleanup of heap-backed integer operands.
heap_count = int('1180591620717411303424')
for sequence in sequences:
    for count in [2**70, -(2**70), heap_count, -heap_count]:
        for reverse in [False, True]:
            try:
                result = count * sequence if reverse else sequence * count
            except OverflowError as exc:
                assert str(exc) == "cannot fit 'int' into an index-sized integer"
            else:
                assert False
    for count, error in [(InvalidCount(), TypeError), (RaisingCount(), ValueError)]:
        try:
            sequence * count
        except error:
            pass
        else:
            assert False


for sequence in sequences:
    for count in [0, -3]:
        expected = sequence * count
        events.clear()
        assert sequence * Count(count) == expected
        assert events == ['index']
        events.clear()
        assert Count(count) * sequence == expected
        assert events == ['index']

for make in [list, deque]:
    for count in [0, -3]:
        sequence = make([1, 2])
        alias = sequence
        events.clear()
        sequence *= Count(count)
        assert sequence is alias and list(sequence) == []
        assert events == ['index']
