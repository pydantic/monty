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

for mutation, expected in [('clear', b''), ('grow', b'Xbcabc')]:
    receiver = bytearray(b'abc')

    class ResizeReceiver:
        def __index__(self):
            if mutation == 'clear':
                receiver.clear()
            else:
                receiver.extend(b'abc')
            return 1

    assert receiver.replace(b'a', b'X', ResizeReceiver()) == expected

for constructor in [bytes, bytearray]:
    for mutation in ['clear_old', 'grow_old', 'clear_new', 'grow_new']:
        old = bytearray(b'a')
        new = bytearray(b'X')

        class ResizeArgument:
            def __index__(self):
                if mutation == 'clear_old':
                    old.clear()
                elif mutation == 'grow_old':
                    old.extend(b'b')
                elif mutation == 'clear_new':
                    new.clear()
                else:
                    new.extend(b'Y')
                return 1

        try:
            constructor(b'abcabc').replace(old, new, ResizeArgument())
        except BufferError as exc:
            assert str(exc) == 'Existing exports of data: object cannot be re-sized'
        else:
            assert False, mutation
        assert old == b'a' and new == b'X'
        old.append(98)
        new.append(89)

receiver = bytearray(b'abc')


class AliasedCount:
    def __index__(self):
        receiver.clear()
        return 1


try:
    receiver.replace(receiver, receiver, AliasedCount())
except BufferError:
    pass
else:
    assert False
receiver.append(33)
assert receiver == b'abc!'


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
