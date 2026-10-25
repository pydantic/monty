def cmp(left, op, right):
    if op == '<':
        return left < right
    if op == '<=':
        return left <= right
    if op == '>':
        return left > right
    if op == '>=':
        return left >= right
    raise ValueError('unknown comparison: ' + op)


for left_type in (set, frozenset):
    for right_type in (set, frozenset):
        for left, right, expected in (
            ([], [], {'<': False, '<=': True, '>': False, '>=': True}),
            ([], [1], {'<': True, '<=': True, '>': False, '>=': False}),
            ([1], [], {'<': False, '<=': False, '>': True, '>=': True}),
            ([1, 2], [2, 1], {'<': False, '<=': True, '>': False, '>=': True}),
            ([1], [1, 2], {'<': True, '<=': True, '>': False, '>=': False}),
            ([1, 2], [1], {'<': False, '<=': False, '>': True, '>=': True}),
            ([1], [2], {'<': False, '<=': False, '>': False, '>=': False}),
            ([1, 2], [2, 3, 4], {'<': False, '<=': False, '>': False, '>=': False}),
            ([1, 2, 3], [2, 4], {'<': False, '<=': False, '>': False, '>=': False}),
            ([frozenset([1])], [frozenset([1]), frozenset([2])], {'<': True, '<=': True, '>': False, '>=': False}),
        ):
            a = left_type(left)
            b = right_type(right)

            for op, result in expected.items():
                assert cmp(a, op, b) == result

    a = left_type([1])
    assert not a < a
    assert a <= a
    assert not a > a
    assert a >= a
    assert a <= {1: 'a'}.keys()
    assert a < {1: 'a', 2: 'b'}.keys()
    assert a >= {1: 'a'}.keys()
    assert a > {}.keys()
    assert left_type([(1, 'a')]) <= {1: 'a'}.items()

    for other in ([1], (1,), {1: 'a'}, '1', 1, None):
        for first, second in ((a, other), (other, a)):
            for op in ('<', '<=', '>', '>='):
                try:
                    cmp(first, op, second)
                except TypeError as exc:
                    expected = (
                        "'"
                        + op
                        + "' not supported between instances of '"
                        + type(first).__name__
                        + "' and '"
                        + type(second).__name__
                        + "'"
                    )
                    assert str(exc) == expected
                else:
                    assert False, 'expected TypeError'


class Key:
    def __init__(self, value):
        self.value = value

    def __hash__(self):
        assert not reject_hash
        return self.value

    def __eq__(self, other):
        return self.value == other.value


reject_hash = False
for left_type in (set, frozenset):
    for right_type in (set, frozenset):
        a = left_type([Key(1)])
        b = right_type([Key(1), Key(2)])
        reject_hash = True
        assert a < b and a <= b and b > a and b >= a
        assert a.issubset(b) and b.issuperset(a)
        assert not b.issubset(a) and not a.issuperset(b)
        reject_hash = False

for collection in (set, frozenset):
    a = collection([1])
    assert {1: 'a'}.keys() <= a
    assert {1: 'a', 2: 'b'}.keys() > a
    assert {}.keys() < a

    a = collection([(1, 'a')])
    assert {1: 'a'}.items() <= a
    assert {}.items() < a
    assert {1: 'a'}.items() >= a
    assert {1: 'a', 2: 'b'}.items() > a

    items = {1: []}.items()
    assert not collection([(1, 2)]) <= items
    assert not collection([(1, 2)]) < items
    assert not collection([(1, 2)]) > items
    assert not collection() >= items


class ExplodingEquality:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        raise ValueError('comparison failed')


for collection in (set, frozenset):
    a = collection([ExplodingEquality()])
    b = collection([ExplodingEquality()])
    assert not a < b
    assert not a > b
    for op in ('<=', '>='):
        try:
            cmp(a, op, b)
        except ValueError as exc:
            assert str(exc) == 'comparison failed'
        else:
            assert False, 'expected ValueError'


for collection in (set, frozenset):
    for method in ('issubset', 'issuperset'):
        if method == 'issubset':
            assert collection(['a']).issubset('a')
            assert collection(['a']).issubset(['a', 'b'])
        else:
            assert collection(['a']).issuperset('a')
            assert collection(['a', 'b']).issuperset(['a'])
        for other in (1, None):
            try:
                if method == 'issubset':
                    collection().issubset(other)
                else:
                    collection().issuperset(other)
            except TypeError:
                pass
            else:
                assert False, 'expected TypeError'

    for left_dict, right_dict, expected in (
        ({}, {}, {'<': False, '<=': True, '>': False, '>=': True}),
        ({}, {1: 'a'}, {'<': True, '<=': True, '>': False, '>=': False}),
        ({1: 'a'}, {}, {'<': False, '<=': False, '>': True, '>=': True}),
        ({1: 'a'}, {1: 'a'}, {'<': False, '<=': True, '>': False, '>=': True}),
        ({1: 'a'}, {2: 'b', 3: 'c'}, {'<': False, '<=': False, '>': False, '>=': False}),
        ({1: 'a', 2: 'b'}, {3: 'c'}, {'<': False, '<=': False, '>': False, '>=': False}),
    ):
        for left_view, right_view in (
            (left_dict.keys(), right_dict.keys()),
            (left_dict.items(), right_dict.items()),
        ):
            for op, result in expected.items():
                assert cmp(collection(left_view), op, right_view) == result
                assert cmp(left_view, op, collection(right_view)) == result

    for op in ('<=', '>='):
        try:
            cmp(collection([ExplodingEquality()]), op, {ExplodingEquality(): 1}.keys())
        except ValueError as exc:
            assert str(exc) == 'comparison failed'
        else:
            assert False, 'expected ValueError'

    for op in ('<=', '>='):
        try:
            cmp({ExplodingEquality(): 1}.keys(), op, collection([ExplodingEquality()]))
        except ValueError as exc:
            assert str(exc) == 'comparison failed'
        else:
            assert False, 'expected ValueError'

    # A successful subset test must not hash unrelated items in the view.
    assert collection([(1, 'a')]) <= {1: 'a', 2: []}.items()
    assert collection([(1, 'a')]) < {1: 'a', 2: []}.items()

    try:
        collection([(1, 'a')]) >= {2: []}.items()
    except TypeError:
        pass
    else:
        assert False, 'expected TypeError'
