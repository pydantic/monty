import re

# === a function as the replacement gets each match ===
assert re.sub('a', lambda m: m.group().upper(), 'banana') == 'bAnAnA'
assert re.sub('a', lambda m: m.group().upper(), 'banana', count=2) == 'bAnAna'
assert re.compile('a').sub(lambda m: 'X', 'banana', 1) == 'bXnana'
assert re.sub(r'(\w)(\d)', lambda m: m.group(2) + m.group(1), 'a1b2') == '1a2b'
assert re.sub('(?P<n>a)', lambda m: m.group('n') * 2, 'abc') == 'aabc'
assert re.sub('a', lambda m: str(m.start()), 'aXa') == '0X2'
assert re.sub('é', lambda m: str(m.span()), 'héé') == 'h(1, 2)(2, 3)'
assert re.sub('a', lambda m: m.string, 'ab') == 'abb'
# the result is used as it is, not as a template
assert re.sub('a', lambda m: '\\1', 'abc') == '\\1bc'
# empty matches, as with a template
assert re.sub('x*', lambda m: '-', 'abc') == '-a-b-c-'
assert re.sub('', lambda m: '_', '') == '_'
# no match calls nothing, and a negative count changes nothing
assert re.sub('z', lambda m: 5, 'abc') == 'abc'
assert re.sub('a', lambda m: 5, 'abc', count=-1) == 'abc'
assert re.compile('a').sub(lambda m: 5, 'abc', count=-1) == 'abc'
# a None result adds nothing
assert re.sub('a', lambda m: None, 'banana') == 'bnn'

# a named function, called in order
seen = []


def record(m):
    seen.append(m.group())
    return str(len(seen))


assert re.sub(r'\d+', record, 'a1b22c333') == 'a1b2c3'
assert seen == ['1', '22', '333']

# === errors ===
# a result that is not a str fails after every call has run, naming the first
# bad item of the pieces CPython joins
calls = []


def bad(m):
    calls.append(m.start())
    return 5 if m.start() == 1 else 'ok'


try:
    re.sub('a', bad, 'xaya')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'sequence item 1: expected str instance, int found'
assert calls == [1, 3]


# an error from the function propagates
def boom(m):
    raise KeyError('boom')


try:
    re.compile('a').sub(boom, 'xa')
    assert False, 'expected KeyError'
except KeyError as e:
    assert e.args == ('boom',)

# the subject must still be a string
try:
    re.sub('a', lambda m: 'x', 5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "expected string or bytes-like object, got 'int'"

# the function gets the match as its one argument
try:
    re.sub('a', lambda: 'x', 'abc')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == '<lambda>() takes 0 positional arguments but 1 was given'
