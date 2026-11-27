import copy
import re
import sys
from collections import defaultdict, namedtuple
from datetime import date, datetime
from io import StringIO
from pathlib import Path

items = []
append = items.append
append(1)
list.append(items, 2)
unbound = list.append
unbound(items, 3)
assert items == [1, 2, 3]
assert append.__self__ is items
assert append.__name__ == 'append'
assert unbound.__objclass__ is list
assert unbound.__qualname__ == 'list.append'
assert unbound is list.append
assert type(append).__name__ == 'builtin_function_or_method'
assert type(unbound).__name__ == 'method_descriptor'
assert not hasattr(append, '__func__')
assert getattr(items, ''.join(['app', 'end']))(4) is None
assert items == [1, 2, 3, 4]

try:
    unbound((), 5)
except TypeError as exc:
    assert str(exc) == "descriptor 'append' for 'list' objects doesn't apply to a 'tuple' object"
else:
    assert False, 'saved list.append accepted a tuple'

assert list(map(str.upper, ['a', 'b'])) == ['A', 'B']
assert 'abc'.upper() == 'ABC'
upper = 'abc'.upper
assert upper() == 'ABC'
assert defaultdict.get is dict.get
factory = defaultdict.fromkeys
assert factory.__self__ is defaultdict
assert type(factory(['x'])) is defaultdict
factory = {}.fromkeys
assert factory.__self__ is dict
assert factory(['a', 'b'], 2) == {'a': 2, 'b': 2}
replace = date.replace
assert replace(datetime(2024, 2, 3), year=2025) == datetime(2025, 2, 3)
stream = StringIO()
write = stream.write
assert write('hi') == 2
assert stream.getvalue() == 'hi'

# The saved method holds the only reference to its list.
pop = [8, 9].pop
assert pop() == 9
assert pop() == 8


search = re.compile('a+').search
group = search('baa').group
assert group() == 'aa'
join = Path('a').joinpath
assert str(join('b')) == 'a/b'
Point = namedtuple('Point', 'count x')
point = Point(7, 3)
assert point.count == 7
as_dict = point._asdict
assert as_dict() == {'count': 7, 'x': 3}
assert not hasattr(sys.version_info, '_asdict')


assert copy.copy(append) is append
assert copy.deepcopy(append) is append
assert append == items.append
assert hash(append) == hash(items.append)
assert append != [].append
assert factory.__qualname__ == 'dict.fromkeys'
assert list.append.__get__(items, list) == append
assert list.append.__get__(None, list) is list.append
assert type(list.append).__get__(list.append, items) == append


class MethodHolder:
    member = list.append


assert MethodHolder.member is list.append
try:
    MethodHolder().member
except TypeError as exc:
    assert "descriptor 'append'" in str(exc)
else:
    assert False, 'list.append bound an unrelated instance'
