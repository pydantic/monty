import bisect
import random

for sequence in [bytearray([1, 3, 3, 7]), memoryview(b'\x01\x03\x03\x07')]:
    assert bisect.bisect_left(sequence, 3) == 1
    assert bisect.bisect_right(sequence, 3) == 3
    assert bisect.bisect_left(sequence, 6, key=lambda item: item * 2) == 1
    for k in [0, 1, 3]:
        rng = random.Random(77)
        reference = random.Random(77)
        assert rng.sample(sequence, k) == reference.sample(list(sequence), k)
        assert rng.getstate() == reference.getstate()

mutable = bytearray([1, 3, 7])
bisect.insort(mutable, 5)
assert mutable == b'\x01\x03\x05\x07'
held = memoryview(mutable)
try:
    bisect.insort_left(mutable, 3)
except BufferError as exc:
    assert str(exc) == 'Existing exports of data: object cannot be re-sized'
else:
    assert False
held.release()
bisect.insort_left(mutable, 3)
assert mutable == b'\x01\x03\x03\x05\x07'

for code in ['B', 'H', 'c', 'd', '?']:
    size = {'B': 1, 'H': 2, 'c': 1, 'd': 8, '?': 1}[code]
    for n in [0, 2, 64]:
        base = memoryview(bytearray(max(n, 1) * 2 * size)).cast(code, [max(n, 1), 2])
        population = base if n else base[:0]
        for k in [0, 1, 2]:
            rng = random.Random(77)
            reference = random.Random(77)
            # Pool construction fails before drawing; the large-set path draws once before indexing.
            if n > 21 and k:
                reference.randrange(n)
            try:
                result = rng.sample(population, k)
            except ValueError as exc:
                assert k > n
                assert str(exc) == 'Sample larger than population or is negative'
            except NotImplementedError as exc:
                assert n <= 21 or k > 0
                assert str(exc) == 'multi-dimensional sub-views are not implemented'
            else:
                assert n > 21 and k == 0 and result == []
            assert rng.getstate() == reference.getstate(), (code, n, k)

for code in 'Bbc?hHiIlLqQnNefdP':
    size = {'B': 1, 'b': 1, 'c': 1, '?': 1, 'h': 2, 'H': 2, 'e': 2, 'i': 4, 'I': 4, 'f': 4}.get(code, 8)
    owner = bytearray([i % 64 for i in range(16 * size)])
    view = memoryview(owner).cast(code)[::2]
    reference = view.tolist()
    rng = random.Random(77)
    reference_rng = random.Random(77)
    rng.shuffle(view)
    reference_rng.shuffle(reference)
    assert view.tolist() == reference, code
    assert rng.getstate() == reference_rng.getstate(), code

    readonly = view.toreadonly()
    rng = random.Random(77)
    reference_rng = random.Random(77)
    reference_rng.randrange(len(readonly))
    try:
        rng.shuffle(readonly)
    except TypeError as exc:
        assert str(exc) == 'cannot modify read-only memory'
    else:
        assert False
    assert rng.getstate() == reference_rng.getstate(), code

mutable = bytearray(range(10))
reference = list(mutable)
rng = random.Random(77)
reference_rng = random.Random(77)
rng.shuffle(mutable)
reference_rng.shuffle(reference)
assert list(mutable) == reference
assert rng.getstate() == reference_rng.getstate()

# Failed choices lookups consume only the first draw.
for population in [bytearray(), memoryview(b''), memoryview(b'abcd').cast('B', [2, 2])]:
    for weighted in [False, True]:
        if weighted and len(population) == 0:
            continue
        rng = random.Random(77)
        expected = random.Random(77)
        expected.random()
        try:
            rng.choices(population, k=6, weights=[1] * len(population) if weighted else None)
        except (IndexError, NotImplementedError):
            pass
        else:
            assert False
        assert rng.getstate() == expected.getstate()
