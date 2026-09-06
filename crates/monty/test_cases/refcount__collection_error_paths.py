first = (1,)
last = (3,)
value = [2]

try:
    {1: first, []: value, 3: last}
    assert False, 'expected an unhashable key to raise'
except TypeError as exc:
    assert str(exc) == "cannot use 'list' as a dict key (unhashable type: 'list')"

try:
    {first, [], last}
    assert False, 'expected an unhashable element to raise'
except TypeError as exc:
    assert str(exc) == "cannot use 'list' as a set element (unhashable type: 'list')"


def check_constructor(constructor):
    try:
        constructor([first, [], last])
        assert False, 'expected an unhashable element to raise'
    except TypeError as exc:
        assert str(exc) == "cannot use 'list' as a set element (unhashable type: 'list')"


check_constructor(set)
check_constructor(frozenset)


def update(items):
    target = set()
    target.update(items)


check_constructor(update)


class Key:
    def __init__(self, number):
        self.number = number

    def __hash__(self):
        return 0

    def __eq__(self, other):
        if self.number == 99 and other.number == 2:
            raise ValueError('comparison failed')
        return self.number == other.number


def check_update(constructor):
    source = constructor([Key(1), Key(2), Key(3)])
    target = {Key(99)}
    try:
        target.update(source)
        assert False, 'expected comparison to fail'
    except ValueError as exc:
        assert str(exc) == 'comparison failed'


check_update(set)
check_update(frozenset)
check_update(list)

# ref-counts={'first': 1, 'last': 1, 'value': 1, 'Key': 1}
