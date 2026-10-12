use monty::MontyRun;
use monty_types::CompileOptions;

#[test]
fn tagged_types_preserve_payload_dependent_types() {
    let code = r#"
from collections import Counter, defaultdict, namedtuple
from functools import partial
from hashlib import sha256
from datetime import date, datetime

class Example:
    pass

Point = namedtuple('Point', ['x'])
values = [([], list), ({}, dict), (set(), set), (frozenset(), frozenset),
          (Counter(), Counter), (defaultdict(int), defaultdict),
          (Point(1), Point), (Example(), Example), (date(2026, 1, 1), date),
          (datetime(2026, 1, 1), datetime)]
for value, expected in values:
    assert type(value) is expected
assert str(type(ValueError())) == "<class 'ValueError'>"
assert str(type(TypeError())) == "<class 'TypeError'>"
assert str(type(sha256())) == "<class '_hashlib.HASH'>"
assert type(partial(Example)()) is Example
assert partial(lambda: 42)() == 42
try:
    partial([])
except TypeError:
    pass
else:
    assert False
"#;
    let mut run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    run.run_no_limits(vec![]).unwrap();
}

#[test]
fn tag_fast_paths_preserve_iteration_formatting_and_conversions() {
    let code = r"
from base64 import a85encode
from binascii import hexlify
from datetime import date

assert next(iter([10, 20])) == 10
assert next(iter([]), 42) == 42
assert format([1, 2], '') == '[1, 2]'
assert format(date(2026, 1, 2), '%Y-%m-%d') == '2026-01-02'
assert (10, 20, 30)[1:] == (20, 30)
class Index:
    def __index__(self):
        return 1
assert (10, 20, 30)[Index()] == 20
assert a85encode(b'hello', wrapcol=10**30) == a85encode(b'hello')
assert hexlify(b'ab', '-'.encode()) == b'61-62'
class Example:
    pass
assert str(Example | None).endswith('Example | None')
assert str(list[int] | str) == 'list[int] | str'
try:
    hexlify(b'ab', [1])
except TypeError:
    pass
else:
    assert False
";
    let mut run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    run.run_no_limits(vec![]).unwrap();
}

#[test]
fn tagged_filters_preserve_mixed_types_and_protocols() {
    let code = r"
from collections import deque, namedtuple
from datetime import date, datetime, time, timedelta, timezone
from copy import copy, deepcopy
import json

Point = namedtuple('Point', ['x'])
assert Point(1) == (1,)
assert (1,) == Point(1)
assert Point(1) < (2,)
assert (1,) < Point(2)
values = [[1], (1,), {1}, {'x': 1}, deque([1]), date(2026, 1, 1),
          datetime(2026, 1, 1), time(1), timedelta(days=1),
          timezone.utc, range(1), slice(1), 'heap-' + str(123),
          'bytes'.encode(), 10 ** 30]
for i, left in enumerate(values):
    assert left == left
    for j, right in enumerate(values):
        if i != j:
            assert left != right
assert copy(values[0]) == [1]
assert deepcopy({'x': values[0]}) == {'x': [1]}
assert json.loads('[]') == []
for value in values:
    if isinstance(value, str):
        assert 'abc'.startswith(value) is False
    else:
        try:
            'abc'.startswith(value)
        except TypeError:
            pass
        else:
            assert False

class Indexed:
    def __index__(self):
        return 1

class Equal:
    def __eq__(self, other):
        return True

assert [1, 2][Indexed()] == 2
assert Equal() == [1]
assert [1] == Equal()
class Iterable:
    def __iter__(self):
        return iter([1, 2])
assert [*Iterable()] == [1, 2]
for value in [iter([1]), iter((1,)), iter('a'), iter(b'a'), iter(range(1))]:
    assert next(iter(value)) is not None
";
    let mut run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    run.run_no_limits(vec![]).unwrap();
}
