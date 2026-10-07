events = []


class C:
    def method(self, *args, **kwargs):
        events.append('original')
        return args, kwargs


x = C()
original = C.method


def receiver():
    events.append('receiver')
    return x


def replace():
    events.append('argument')
    C.method = lambda self, *args, **kwargs: 'replacement'
    return 7


assert receiver().method(replace()) == ((7,), {})
assert events == ['receiver', 'argument', 'original']
assert x.method(7) == 'replacement'

# Every call form saves the member before arguments can replace it.
C.method = original
assert x.method(value=replace()) == ((), {'value': 7})
C.method = original
assert x.method(1, value=replace()) == ((1,), {'value': 7})
C.method = original
assert x.method(*[replace()]) == ((7,), {})
C.method = original
assert x.method(**{'value': replace()}) == ((), {'value': 7})
C.method = original
assert x.method(*[replace()], *[2], **{'a': 3}, **{'b': 4}) == ((7, 2), {'a': 3, 'b': 4})
C.method = original
assert C.method(x, replace()) == ((7,), {})

# Instance attributes remain unbound and are also saved before arguments.
x.method = lambda value: ('instance', value)


def replace_instance():
    x.method = lambda value: 'replacement'
    return 9


assert x.method(replace_instance()) == ('instance', 9)
assert x.method(9) == 'replacement'

# Missing attributes fail before evaluating arguments.
events.clear()
try:
    x.missing(replace())
except AttributeError:
    pass
else:
    assert False
assert events == []

# A non-callable attribute is retrieved first but rejected after arguments run.
x.method = 42
events.clear()
try:
    x.method(replace())
except TypeError:
    pass
else:
    assert False
assert events == ['argument']

# Native calls retain positional, keyword and unpacked argument dispatch.
xs = []
xs.append(1)
xs.extend(*[[2, 3]])
xs.sort(reverse=True)
xs.sort(**{'reverse': True})
assert xs == [3, 2, 1]
try:
    xs.sort(**{'reverse': True}, **{'reverse': False})
except TypeError as exc:
    assert str(exc) == "list.sort() got multiple values for keyword argument 'reverse'"
else:
    assert False
