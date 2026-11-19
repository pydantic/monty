def pair():
    return [1], [2]


a, b = pair()
assert a == [1] and b == [2]
shared = []
for values in [(shared, shared), [shared, shared]]:
    a, b = values
    assert a is shared and b is shared
    a.append(42)
    assert b is values[0]

for values in [(), [], (1,), [1], (1, 2, 3), [1, 2, 3]]:
    a, b = 'old-a', 'old-b'
    try:
        a, b = values
    except ValueError as exc:
        if len(values) < 2:
            assert str(exc) == f'not enough values to unpack (expected 2, got {len(values)})'
        else:
            assert str(exc) == 'too many values to unpack (expected 2, got 3)'
    else:
        assert False
    assert a == 'old-a' and b == 'old-b'

() = ()
() = []
(a,) = [shared]
assert a is shared
a, (b, c) = (shared, ([1], [2]))
assert a is shared and b == [1] and c == [2]
store = [None]
try:
    store[0], store[5] = pair()
except IndexError:
    pass
assert store == [[1]]

# Generic iterators still stop at the first surplus item.
events = []


class Source:
    def __iter__(self):
        return self

    def __next__(self):
        events.append(len(events))
        return events[-1]


try:
    a, b = Source()
except ValueError as exc:
    assert str(exc) == 'too many values to unpack (expected 2)'
assert events == [0, 1, 2]

# Exceed the initial operand stack capacity with shared heap values.
shared = [42]
for source in ((shared,) * 70, [shared] * 70):
    (
        item0,
        item1,
        item2,
        item3,
        item4,
        item5,
        item6,
        item7,
        item8,
        item9,
        item10,
        item11,
        item12,
        item13,
        item14,
        item15,
        item16,
        item17,
        item18,
        item19,
        item20,
        item21,
        item22,
        item23,
        item24,
        item25,
        item26,
        item27,
        item28,
        item29,
        item30,
        item31,
        item32,
        item33,
        item34,
        item35,
        item36,
        item37,
        item38,
        item39,
        item40,
        item41,
        item42,
        item43,
        item44,
        item45,
        item46,
        item47,
        item48,
        item49,
        item50,
        item51,
        item52,
        item53,
        item54,
        item55,
        item56,
        item57,
        item58,
        item59,
        item60,
        item61,
        item62,
        item63,
        item64,
        item65,
        item66,
        item67,
        item68,
        item69,
    ) = source
    assert item0 is shared
    assert item35 is shared
    assert item69 is shared
