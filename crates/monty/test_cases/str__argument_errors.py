import math
import random
import re


def raises(error_type, call):
    try:
        call()
    except error_type:
        return
    assert False, 'expected an argument error'


short = 'abc!'[:-1]
long = 'abcdefghijklmnop!'[:-1]
for invalid in [short, long, [], {}]:
    raises(TypeError, lambda: b'abc'.startswith((b'zz', invalid)))
    raises(TypeError, lambda: b'abc'.endswith((invalid, b'c')))
    raises(TypeError, lambda: b'abc'.startswith(invalid))
    raises(TypeError, lambda: b'abc'.find(invalid))
    raises(TypeError, lambda: b'abc'.count(invalid))

raises(TypeError, lambda: b'abc'.startswith(42))

for separator in ['', short, long, 'é!'[:-1], b'', b'::', b'\xff']:
    raises(ValueError, lambda: b'ab'.hex(separator))
raises(TypeError, lambda: b'ab'.hex(42))
raises(TypeError, lambda: bytes.fromhex(42))
assert b'abc'.startswith((b'zz', b'ab'))
assert b'abc'.endswith((b'zz', b'bc'))
assert b'ab'.hex(b':') == '61:62'
assert bytes.fromhex(('61 ' * 8)[:-1]) == b'a' * 8

pattern = re.compile('a')
for invalid in [None, 42, [], {}]:
    for operation in [
        lambda: pattern.search(invalid),
        lambda: pattern.match(invalid),
        lambda: pattern.fullmatch(invalid),
        lambda: pattern.findall(invalid),
        lambda: pattern.finditer(invalid),
    ]:
        raises(TypeError, operation)
    raises(TypeError, lambda: pattern.sub(short, invalid))
    raises(TypeError, lambda: pattern.sub(short, invalid, -1))
    raises(TypeError, lambda: pattern.split(invalid))
raises(TypeError, lambda: pattern.sub(42, short))

rng = random.Random(0)
state = rng.getstate()
raises(ValueError, lambda: rng.setstate((short,) + state[1:]))
assert rng.getstate() == state

for path in [None, [], {}]:
    raises(TypeError, lambda: open(path))
raises(TypeError, lambda: open(short, 42))
raises(ValueError, lambda: open(short, 'rr!'[:-1]))
raises(ValueError, lambda: format(short, '+!'[:-1]))

for operation in [
    lambda: short.find(42),
    lambda: short.replace(short, 42),
    lambda: short.split(42),
    lambda: short.removeprefix(42),
]:
    raises(TypeError, operation)


class BadString:
    def __str__(self):
        return 42

    def __repr__(self):
        return 42


for operation in [
    lambda: f'{BadString()}',
    lambda: f'{BadString()!s}',
    lambda: f'{BadString()!r}',
    lambda: f'{BadString()!a}',
    lambda: '{!s}'.format(BadString()),
    lambda: '{!r}'.format(BadString()),
]:
    raises(TypeError, operation)


import csv
import unicodedata


def match_group(index):
    match = re.search('(a)', 'a')
    assert match is not None
    return match[index]


for invalid in [None, 1.5, b'a', []]:
    raises(IndexError, lambda: match_group(invalid))
for invalid in [None, 42, []]:
    raises(TypeError, lambda: unicodedata.lookup(invalid))


class Sink:
    def write(self, text):
        return len(text)


for invalid in [None, 42, [], {}]:
    raises(AttributeError, lambda: csv.DictWriter(Sink(), ['column'], extrasaction=invalid))


from pathlib import Path

for name in ['short!'[:-1], 'keyword_longer_than_short_storage!'[:-1]]:
    try:
        Path.cwd(**{name: 1})
    except TypeError as error:
        assert str(error).endswith(f"got an unexpected keyword argument '{name}'")
    else:
        assert False, 'expected an unexpected-keyword error'

for receiver in [math, list[int]]:
    for name in ['missing_method!'[:-1], 'missing_method_longer_than_short!'[:-1]]:
        raises(AttributeError, lambda: getattr(receiver, name))

try:
    Path.cwd(**{42: 1})
except TypeError as error:
    assert str(error) == 'keywords must be strings'
else:
    assert False, 'expected a non-string-key error'
assert Path.cwd(**{}) == Path.cwd()


# Dynamic specs format each component before applying the spec to the outer value.
class BadSpec:
    def __str__(self):
        raise ValueError('bad spec')


raises(ValueError, lambda: f'{42:{BadSpec()}}')
for text in [short, long]:
    assert text.find('a') == 0
    assert text.find('a!'[:-1]) == 0
    assert text.rfind('a') == 0
    assert text.count('a') == 1


def accept_keywords(**kwargs):
    return kwargs


raises(TypeError, lambda: accept_keywords(existing=1, **{42: 2}))
