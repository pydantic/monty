# Search boundaries use Python character positions, including empty needles.
for text in ('', 'abcdef', 'äbc中文🐍def', 'aé中🐍' * 200):
    characters = list(text)
    for needle in ('', 'a', '中', '中🐍', 'not-present'):
        for start in (-10000, -10, -1, 0, 1, 6, 10000):
            for stop in (-10000, -1, 0, 1, 8, 10000):
                low = max(len(text) + start, 0) if start < 0 else start
                high = max(len(text) + stop, 0) if stop < 0 else min(stop, len(text))
                matches = []
                if low <= high:
                    for pos in range(low, high + 1):
                        if pos + len(needle) <= high and ''.join(characters[pos : pos + len(needle)]) == needle:
                            matches.append(pos)
                assert text.find(needle, start, stop) == (matches[0] if matches else -1)
                assert text.rfind(needle, start, stop) == (matches[-1] if matches else -1)
                expected_start = (
                    low <= high and low + len(needle) <= high and ''.join(characters[low : low + len(needle)]) == needle
                )
                expected_end = (
                    low <= high
                    and high - len(needle) >= low
                    and ''.join(characters[high - len(needle) : high]) == needle
                )
                assert text.startswith(needle, start, stop) == expected_start
                assert text.endswith(needle, start, stop) == expected_end
                if not needle:
                    assert text.count(needle, start, stop) == len(matches)
    assert text.find('not-present') == -1
    assert text.count('not-present') == 0
    assert text.find('') == 0
    assert text.rfind('') == len(text)
    assert text.count('') == len(text) + 1


class Boundary:
    def __index__(self):
        # Re-enter the same string while its search arguments are converted.
        assert text.find('中') == 2
        assert text[-2] == '中'
        return 4


text = 'aé中🐍' * 1000
assert text.find('中', Boundary()) == 6
assert text.rfind('中', 0, Boundary()) == 2

assert text.startswith('中', Boundary()) is False
assert text.endswith('🐍', 0, Boundary())
assert text.startswith(('aé', 123))
assert text.endswith(('中🐍', 123))


def starts(affix, start=0):
    return text.startswith(affix, start)


def ends(affix, start=0):
    return text.endswith(affix, start)


for method in (starts, ends):
    for prefix in (123, ('not-present', 123)):
        try:
            method(prefix)
        except TypeError:
            pass
        else:
            assert False
    try:
        method((123,), len(text) + 1)
    except TypeError:
        pass
    else:
        assert False


class BadBoundary:
    def __index__(self):
        raise ValueError('boundary first')


for method in (starts, ends):
    try:
        method(123, BadBoundary())
    except ValueError as error:
        assert str(error) == 'boundary first'
    else:
        assert False
