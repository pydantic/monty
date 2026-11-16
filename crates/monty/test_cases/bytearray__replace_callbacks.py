for constructor in [bytes, bytearray]:
    for mutate in ['old', 'new']:
        old = bytearray(b'a')
        new = bytearray(b'X')

        class Count:
            def __index__(self):
                if mutate == 'old':
                    old[0] = 98
                else:
                    new[0] = 89
                return 1

        result = constructor(b'abc').replace(old, new, Count())
        assert result == (b'aXc' if mutate == 'old' else b'Ybc')
        assert type(result) is constructor


class BadCount:
    def __index__(self):
        raise ValueError('count ran')


for old, new in [('a', b'b'), (b'a', 'b')]:
    try:
        bytearray(b'abc').replace(old, new, BadCount())
    except TypeError:
        pass
    else:
        assert False
