"""UTF-8 reads, slices and searches across text sizes and character widths."""


def check(text):
    expected = list(text)
    length = len(expected)
    state = 123
    for _ in range(4096):
        state = (state * 1664525 + 1013904223) & 0xFFFFFFFF
        position = (state >> 8) % length
        assert text[position] == expected[position]
        assert text[position - length] == expected[position]
        end = min(position + 19, length)
        assert text[position:end] == ''.join(expected[position:end])
    assert text[:] == text
    assert text[::-1] == ''.join(reversed(expected))
    assert text[::7] == ''.join(expected[::7])
    assert text[-length] == expected[0]
    assert text[-1] == expected[-1]
    assert text[-length - 1 : -length] == ''
    for position in (-length - 1, length, -(2**63), 2**63 - 1, 10**20, -(10**20)):
        try:
            text[position]
        except IndexError:
            pass
        else:
            assert False, 'out-of-range string index succeeded'


# Literal storage, stack-inline storage and dynamically allocated owners share semantics.
check('aé中🐍\0\u0301\U0010ffffabcdefghijklmnop')
check('é中🐍')
for size in (64, 4096, 65536):
    check(('abcdefghié' * (size // 10 + 1))[:size])
    check(('aé中🐍' * (size // 4 + 1))[:size])


source = 'prefix café 中 🐍 suffix ' * 3
original = source


class RebindIndex:
    def __index__(self):
        global source
        source = 'replacement'
        temporary = [['force heap allocation', index] for index in range(100)]
        assert len(temporary) == 100
        return 9


# Coercion may rebind the source and collect garbage while the original receiver stays guarded.
assert source[RebindIndex()] == original[9]
assert source == 'replacement'


paragraphs = [str(index) + ': café 中 🐍 by the quiet coast' for index in range(128)]
document = '\n\n'.join(paragraphs)
for _ in range(5):
    start = 0
    for index, paragraph in enumerate(paragraphs):
        end = start + len(paragraph)
        assert document.find(paragraph, start) == start
        assert document.index(paragraph, start) == start
        assert document.rfind(paragraph, 0, end) == start
        assert document.rindex(paragraph, 0, end) == start
        assert document.startswith(paragraph, start)
        assert document.endswith(paragraph, start, end)
        assert document.count('é', start, end) == 1
        assert document.find('\n\n', start) == (end if index < 127 else -1)
        start = end + 2

source = original
assert source.find('café', RebindIndex()) > 0
assert source == 'replacement'
