import asyncio
import base64
import datetime
import math
import binascii
import bisect
import copy
import csv
import json
import re
import sys
import unicodedata
from decimal import Decimal
from pathlib import Path

# A keyword built at runtime binds to the same slot as a literal keyword.
assert base64.b64decode(**{'S'.lower(): '-w==', ''.join(['alt', 'chars']): b'-_'}) == b'\xfb'
assert list(enumerate(start=4, iterable='ab')) == [(4, 'a'), (5, 'b')]
assert dict([('x', 1)], iterable=2) == {'x': 1, 'iterable': 2}


class JsonFile:
    def read(self):
        return '[]'

    def write(self, value):
        pass


def _error_message(*, monty, cpython):
    return monty if sys.platform == 'monty' else cpython


# These cases assert each runtime's exact wording and error order.
for call, expected in [
    (
        lambda: len(),
        _error_message(
            monty='len() takes exactly 1 argument (0 given)',
            cpython='len() takes exactly one argument (0 given)',
        ),
    ),
    (
        lambda: len([], []),
        _error_message(
            monty='len() takes exactly 1 argument (2 given)',
            cpython='len() takes exactly one argument (2 given)',
        ),
    ),
    (
        lambda: len([], bogus=1),
        _error_message(
            monty='len() takes no keyword arguments',
            cpython='len() takes no keyword arguments',
        ),
    ),
    (
        lambda: len(*[[]], **{'bogus': 1}),
        _error_message(
            monty='len() takes no keyword arguments',
            cpython='len() takes no keyword arguments',
        ),
    ),
    (
        lambda: math.dist(),
        _error_message(
            monty='math.dist() takes exactly 2 arguments (0 given)',
            cpython='dist expected 2 arguments, got 0',
        ),
    ),
    (
        lambda: math.dist(p=[], q=[]),
        _error_message(
            monty='math.dist() takes no keyword arguments',
            cpython='math.dist() takes no keyword arguments',
        ),
    ),
    (
        lambda: [].insert(index=0, object=1),
        _error_message(
            monty='list.insert() takes no keyword arguments',
            cpython='list.insert() takes no keyword arguments',
        ),
    ),
    (
        lambda: [].pop(0, 1),
        _error_message(
            monty='list.pop() takes at most 1 argument (2 given)',
            cpython='pop expected at most 1 argument, got 2',
        ),
    ),
    (
        lambda: {}.get(),
        _error_message(
            monty='get() takes at least 1 argument (0 given)',
            cpython='get expected at least 1 argument, got 0',
        ),
    ),
    (
        lambda: [].clear(1),
        _error_message(
            monty='list.clear() takes no arguments (1 given)',
            cpython='list.clear() takes no arguments (1 given)',
        ),
    ),
    (
        lambda: enumerate(),
        _error_message(
            monty="enumerate() missing 1 required positional argument: 'iterable'",
            cpython="enumerate() missing required argument 'iterable'",
        ),
    ),
    (
        lambda: enumerate([], 1, 2),
        _error_message(
            monty='enumerate() takes at most 2 arguments (3 given)',
            cpython='enumerate() takes at most 2 arguments (3 given)',
        ),
    ),
    (
        lambda: enumerate([], **{'ITERABLE'.lower(): []}),
        _error_message(
            monty="enumerate() got multiple values for argument 'iterable'",
            cpython="'iterable' is an invalid keyword argument for enumerate()",
        ),
    ),
    (
        lambda: enumerate([], bogus=1),
        _error_message(
            monty="enumerate() got an unexpected keyword argument 'bogus'",
            cpython="'bogus' is an invalid keyword argument for enumerate()",
        ),
    ),
    (
        lambda: math.prod(iterable=[]),
        _error_message(
            monty="math.prod() got some positional-only arguments passed as keyword arguments: 'iterable'",
            cpython='prod() takes exactly 1 positional argument (0 given)',
        ),
    ),
    (
        lambda: math.prod([], 1),
        _error_message(
            monty='math.prod() takes at most 1 positional argument (2 given)',
            cpython='prod() takes exactly 1 positional argument (2 given)',
        ),
    ),
    (
        lambda: math.prod([], 1, start=2),
        _error_message(
            monty='math.prod() takes at most 1 positional argument (2 given)',
            cpython='prod() takes at most 2 arguments (3 given)',
        ),
    ),
    (
        lambda: print(1, 2, bogus=1),
        _error_message(
            monty="print() got an unexpected keyword argument 'bogus'",
            cpython="print() got an unexpected keyword argument 'bogus'",
        ),
    ),
    (
        lambda: datetime.date('bad', 1, day=1, year=2),
        _error_message(
            monty="date() got multiple values for argument 'year'",
            cpython='function takes at most 3 arguments (4 given)',
        ),
    ),
    (
        lambda: len(bogus=[]),
        _error_message(
            monty='len() takes no keyword arguments',
            cpython='len() takes no keyword arguments',
        ),
    ),
    (
        lambda: pow(2),
        _error_message(
            monty='pow() takes at least 2 arguments (1 given)',
            cpython="pow() missing required argument 'exp' (pos 2)",
        ),
    ),
    (
        lambda: reversed([1], [2]),
        _error_message(
            monty='reversed() takes exactly 1 argument (2 given)',
            cpython='reversed expected 1 argument, got 2',
        ),
    ),
    (
        lambda: sum([1], 2, start=3),
        _error_message(
            monty="sum() got multiple values for argument 'start'",
            cpython='sum() takes at most 2 arguments (3 given)',
        ),
    ),
    (
        lambda: round(),
        _error_message(
            monty="round() missing 1 required positional argument: 'number'",
            cpython="round() missing required argument 'number' (pos 1)",
        ),
    ),
    (
        lambda: round(1, 2, 3),
        _error_message(
            monty='round() takes at most 2 arguments (3 given)',
            cpython='round() takes at most 2 arguments (3 given)',
        ),
    ),
    (
        lambda: round(1, bogus=True),
        _error_message(
            monty="round() got an unexpected keyword argument 'bogus'",
            cpython="round() got an unexpected keyword argument 'bogus'",
        ),
    ),
    (
        lambda: next(),
        _error_message(
            monty='next() takes at least 1 argument (0 given)',
            cpython='next expected at least 1 argument, got 0',
        ),
    ),
    (
        lambda: next(iter([]), bogus=True),
        _error_message(
            monty='next() takes no keyword arguments',
            cpython='next() takes no keyword arguments',
        ),
    ),
    (
        lambda: complex(1, 2, imag=3),
        _error_message(
            monty="complex() got multiple values for argument 'imag'",
            cpython='complex() takes at most 2 arguments (3 given)',
        ),
    ),
    (
        lambda: datetime.datetime(2024, 1, 1, 0, 0, 0, 0, None, 0, 1),
        _error_message(
            monty='datetime() takes at most 8 positional arguments (10 given)',
            cpython='function takes at most 9 arguments (10 given)',
        ),
    ),
    (
        lambda: math.sumprod(p=[], q=[]),
        _error_message(
            monty='math.sumprod() takes no keyword arguments',
            cpython='math.sumprod() takes no keyword arguments',
        ),
    ),
    (
        lambda: math.fma(x=1, y=2, z=3),
        _error_message(
            monty='math.fma() takes no keyword arguments',
            cpython='math.fma() takes no keyword arguments',
        ),
    ),
    (
        lambda: binascii.crc32(data=b'a'),
        _error_message(
            monty='binascii.crc32() takes no keyword arguments',
            cpython='binascii.crc32() takes no keyword arguments',
        ),
    ),
    (
        lambda: binascii.crc_hqx(data=b'a', crc=1),
        _error_message(
            monty='binascii.crc_hqx() takes no keyword arguments',
            cpython='binascii.crc_hqx() takes no keyword arguments',
        ),
    ),
    (
        lambda: unicodedata.name(chr='A'),
        _error_message(
            monty='unicodedata.name() takes no keyword arguments',
            cpython='unicodedata.name() takes no keyword arguments',
        ),
    ),
    (
        lambda: bisect.bisect_left([], 0, bogus=True),
        _error_message(
            monty="bisect_left() got an unexpected keyword argument 'bogus'",
            cpython="bisect_left() got an unexpected keyword argument 'bogus'",
        ),
    ),
    (
        lambda: base64.b64decode('YQ==', s='Yg=='),
        _error_message(
            monty="b64decode() got multiple values for argument 's'",
            cpython="b64decode() got multiple values for argument 's'",
        ),
    ),
    (
        lambda: base64.b64encode(b'a', None, 3),
        _error_message(
            monty='b64encode() takes at most 2 arguments (3 given)',
            cpython='b64encode() takes from 1 to 2 positional arguments but 3 were given',
        ),
    ),
    (
        lambda: copy.copy([], bogus=1),
        _error_message(
            monty="copy() got an unexpected keyword argument 'bogus'",
            cpython="copy() got an unexpected keyword argument 'bogus'",
        ),
    ),
    (
        lambda: json.load(JsonFile(), bogus=1),
        _error_message(
            monty="json.load() got an unexpected keyword argument 'bogus'",
            cpython="JSONDecoder.__init__() got an unexpected keyword argument 'bogus'",
        ),
    ),
    (
        lambda: json.dump([], JsonFile(), bogus=1),
        _error_message(
            monty="json.dump() got an unexpected keyword argument 'bogus'",
            cpython="JSONEncoder.__init__() got an unexpected keyword argument 'bogus'",
        ),
    ),
    (
        lambda: re.split('x', 'x', pattern='x'),
        _error_message(
            monty="split() got multiple values for argument 'pattern'",
            cpython="split() got multiple values for argument 'pattern'",
        ),
    ),
    (
        lambda: map(lambda x: x),
        _error_message(
            monty='map() takes at least 2 arguments (1 given)',
            cpython='map() must have at least two arguments.',
        ),
    ),
    (
        lambda: csv.reader(bogus=1),
        _error_message(
            monty="reader() got an unexpected keyword argument 'bogus'",
            cpython='reader expected at least 1 argument, got 0',
        ),
    ),
    (
        lambda: csv.writer(bogus=1),
        _error_message(
            monty="writer() got an unexpected keyword argument 'bogus'",
            cpython='writer expected at least 1 argument, got 0',
        ),
    ),
    (
        lambda: Decimal('1').exp(bogus=1),
        _error_message(
            monty="Decimal.exp() got an unexpected keyword argument 'bogus'",
            cpython="this function got an unexpected keyword argument 'bogus'",
        ),
    ),
    (
        lambda: Path('x').open(file='y'),
        _error_message(
            monty="open() got multiple values for argument 'file'",
            cpython="Path.open() got an unexpected keyword argument 'file'",
        ),
    ),
]:
    try:
        call()
    except TypeError as exc:
        assert str(exc) == expected
    else:
        assert False, 'expected TypeError'


# A valid shape reaches the field converter.
try:
    datetime.date('bad', 1, 1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'str' object cannot be interpreted as an integer"

try:
    enumerate(**{1: []})
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'keywords must be strings'

if sys.platform == 'monty':
    try:
        asyncio.gather(bogus=True)
        assert False, 'expected NotImplementedError'
    except NotImplementedError as exc:
        assert str(exc) == 'gather() does not yet support keyword arguments'


# With one positional argument, sorted() consumes the iterable before binding its options.
class BrokenIterable:
    def __iter__(self):
        raise ValueError('iteration started')


try:
    sorted(BrokenIterable(), bogus=1)
except ValueError as exc:
    assert str(exc) == 'iteration started'
else:
    assert False, 'expected the iteration error'

try:
    sorted(BrokenIterable(), [], bogus=[])
except TypeError as exc:
    assert str(exc) == _error_message(
        monty='sorted() takes exactly 1 argument (2 given)',
        cpython='sorted expected 1 argument, got 2',
    )
else:
    assert False, 'expected the positional count error'


events = []


class RecordingIterable:
    def __iter__(self):
        events.append('iterated')
        return iter([[]])


try:
    sorted(RecordingIterable(), bogus=[])
except TypeError as exc:
    assert events == ['iterated']
    assert str(exc) == _error_message(
        monty="sorted() got an unexpected keyword argument 'bogus'",
        cpython="sort() got an unexpected keyword argument 'bogus'",
    )
else:
    assert False, 'expected the keyword error'


# === 2-arg fast path error cleanup (refcount safety for heap-allocated objects) ===
# 1. Error on arg 1 while arg 2 is a freshly allocated heap object:
try:
    'hello'.replace(123, 'a' * 100)
    assert False, 'replace(non-str, str) should raise TypeError'
except TypeError as exc:
    assert str(exc) == 'replace() argument 1 must be str, not int'

# 2. Error on arg 2 while arg 1 was extracted into an owning field:
try:
    'hello'.replace('a' * 100, 123)
    assert False, 'replace(str, non-str) should raise TypeError'
except TypeError as exc:
    assert str(exc) == 'replace() argument 2 must be str, not int'

# 3. Positional argument count arity checks via fast-path / fallback:
try:
    'hello'.replace()
    assert False, 'replace() with 0 args should raise TypeError'
except TypeError as exc:
    assert str(exc) == _error_message(
        monty="replace() missing 2 required positional arguments: 'old' and 'new'",
        cpython='replace() takes at least 2 positional arguments (0 given)',
    )

try:
    'hello'.replace('a')
    assert False, 'replace() with 1 arg should raise TypeError'
except TypeError as exc:
    assert str(exc) == _error_message(
        monty="replace() missing 1 required positional argument: 'new'",
        cpython='replace() takes at least 2 positional arguments (1 given)',
    )

# 4. Overflow check for ssize_t count:
try:
    'a'.replace('a', 'b', 10**30)
    assert False, 'replace with oversized count should raise OverflowError'
except OverflowError as exc:
    assert str(exc) == 'Python int too large to convert to C ssize_t'

# 5. Invalid count type check (must keep plain __index__ integer wording):
try:
    'a'.replace('a', 'b', 'x')
    assert False, 'replace with non-int count should raise TypeError'
except TypeError as exc:
    assert str(exc) == "'str' object cannot be interpreted as an integer"

try:
    'a'.replace('a', 'b', count='x')
    assert False, 'replace with non-int count kwarg should raise TypeError'
except TypeError as exc:
    assert str(exc) == "'str' object cannot be interpreted as an integer"

# 6. Type name lookup on object without __index__ (preserves exact class name):
from collections import namedtuple

Point = namedtuple('Point', ['x', 'y'])
try:
    'a'.replace('a', 'b', Point(1, 2))
    assert False, 'replace with namedtuple count should raise TypeError'
except TypeError as exc:
    assert str(exc) == "'Point' object cannot be interpreted as an integer"

# 7. 32-bit boundary: count >= 2**31 must not overflow on 32-bit targets
assert 'a'.replace('a', 'b', 2**31) == 'b'
