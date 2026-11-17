import random

data = bytearray(b'A')
match data:
    case bytearray(subject):
        assert subject is data
    case _:
        assert False

assert random.sample(data, 1) == [65]
assert random.Random(7).sample(data, 1) == [65]
assert b'%c' % data == b'A'
assert bytearray(b'%c') % data == bytearray(b'A')


class ChangeByte:
    def __index__(self):
        data[0] = 66
        return 66


assert ChangeByte() in data
assert ChangeByte() in b'B'
assert ChangeByte() in bytes(data)
data[0] = 65
try:
    data[::0] = [ChangeByte()]
except ValueError:
    pass
else:
    assert False
assert data == b'A'


class ChangeTemplate:
    def __int__(self):
        template[-1] = 120
        return 1


template = bytearray(b'%d:%d')
result = template % (ChangeTemplate(), 16)
assert type(result) is bytearray and result == b'1:10'
template.append(33)
