# Compare string access with list access across interned, inline and heap text.
for literal in (
    '',
    'a',
    'abcdefghijk',
    'abcdefghijklmnopqrstuvwxyz0123456789',
    'aé中🐍',
    'aé中🐍aé中🐍aé中🐍aé中🐍aé中🐍aé中🐍aé中🐍aé中🐍aé中🐍aé中🐍',
):
    for text in (literal, ''.join([literal, ''])):
        chars = list(text)
        length = len(chars)
        bounds = (
            None,
            -(10**100),
            -(2**63),
            -length - 1,
            -length,
            -1,
            0,
            1,
            length - 1,
            length,
            length + 1,
            2**63 - 1,
            10**100,
        )
        for start in bounds:
            for stop in bounds:
                for step in (None, -(10**100), -7, -2, -1, 1, 2, 7, 10**100):
                    key = slice(start, stop, step)
                    assert text[key] == ''.join(chars[key]), (text, start, stop, step)

        for index in bounds[1:]:
            if -length <= index < length:
                assert text[index] == chars[index]
            else:
                try:
                    text[index]
                except IndexError:
                    pass
                else:
                    assert False, (text, index)

        try:
            text[::0]
        except ValueError:
            pass
        else:
            assert False


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


for text in ('abcdefghijklmnop', 'aé中🐍aé中🐍aé中🐍aé中🐍aé中🐍aé中🐍aé中🐍aé中🐍aé中🐍aé中🐍'):
    assert text[Index(1)] == text[1]
    assert text[Index(-1)] == text[-1]
    assert text[Index(-9) : Index(-1) : Index(2)] == text[-9:-1:2]


# Repeated contiguous slices exercise the cursor and eventual sparse checkpoints.
text = 'abcdefghü中🐍' * 500
characters = list(text)
for i in range(500):
    start = (i * 7919) % len(text)
    stop = min(start + 73, len(text))
    assert text[start:stop] == ''.join(characters[start:stop])
    assert text[start - len(text) : stop - len(text) or None] == ''.join(characters[start:stop])
assert text[Index(7) : Index(34) : Index(1)] == ''.join(characters[7:34])
