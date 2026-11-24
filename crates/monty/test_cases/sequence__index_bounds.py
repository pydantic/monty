for constructor in [list, tuple]:
    values = constructor([10, 20, 30, 40])
    for start in [-(10**100), -100, -4, -1, 0, 1, 3, 4, 10**100]:
        for end in [-(10**100), -100, -1, 0, 1, 3, 4, 10**100]:
            normalized_start = max(0, min(len(values), start + len(values) if start < 0 else start))
            normalized_end = max(0, min(len(values), end + len(values) if end < 0 else end))
            for target in [10, 20, 30, 40, 50]:
                expected = -1
                for position in range(normalized_start, normalized_end):
                    if values[position] == target:
                        expected = position
                        break
                try:
                    actual = values.index(target, start, end)
                except ValueError:
                    actual = -1
                assert actual == expected

events = []


class Bound:
    def __init__(self, value, label, shrink):
        self.value = value
        self.label = label
        self.shrink = shrink

    def __index__(self):
        events.append(self.label)
        if self.shrink:
            values.clear()
            values.extend([70, 80, 90])
        return self.value


# Bounds can mutate a list; normalize only after both coercions have run.
values = [1, 2, 3, 4, 5]
start = Bound(-2, 'start', True)
end = Bound(20, 'end', False)
assert values.index(80, start, end) == 1
assert events == ['start', 'end']


class Comparable:
    def __init__(self, number, clear):
        self.number = number
        self.clear = clear

    def __eq__(self, other):
        events.append(self.number)
        if self.clear:
            values.clear()
        return False


events = []
values = [Comparable(i, True) for i in range(5)]
try:
    values.index(None, 2)
    assert False, 'expected ValueError'
except ValueError:
    pass
assert events == [2]

for constructor in [list, tuple]:
    events = []
    values = constructor([Comparable(i, False) for i in range(5)])
    try:
        values.index(None, 2, 3)
        assert False, 'expected ValueError'
    except ValueError:
        pass
    assert events == [2]
    events = []
    try:
        values.index(None, 2, 2)
        assert False, 'expected ValueError'
    except ValueError:
        pass
    assert events == []

    key = 'a heap string ' + str(12345)
    values = constructor([key] * 1000)
    assert values.index(key, 999) == 999
    assert values.index(key, -1) == 999
