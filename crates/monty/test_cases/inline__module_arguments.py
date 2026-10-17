import json
import re
from collections import namedtuple
from datetime import date, timedelta, timezone

s = 'abc!'[:-1]
assert json.loads('"ok"!'[:-1]) == 'ok'
assert json.dumps([s]) == '["abc"]'
assert json.dumps({s: s}) == '{"abc": "abc"}'
assert json.dumps([s], indent='--!'[:-1]) == '[\n--"abc"\n]'
assert json.dumps({s: s}, separators=(', !'[:-1], ': !'[:-1])) == '{"abc": "abc"}'
assert json.dumps({s: 1}, sort_keys=True) == '{"abc": 1}'
assert json.dumps('é!'[:-1], ensure_ascii=False) == '"é"'
match = re.search('(?P<word>abc)', s)
assert match is not None
name = 'word!'[:-1]
assert match[name] == s and match.group(name) == s
assert match.group(name, name) == (s, s)
try:
    match['absent!'[:-1]]
    assert False
except IndexError:
    pass
assert timezone(timedelta(hours=1), 'CET!'[:-1]).tzname(None) == 'CET'
try:
    date.fromisoformat('bad!'[:-1])
    assert False
except ValueError:
    pass
Point = namedtuple('Point!'[:-1], ['xx!'[:-1], 'yy!'[:-1]])
point = Point(1, 2)
assert Point._fields == ('xx', 'yy')
assert point._asdict() == {'xx': 1, 'yy': 2}
assert point._replace(**{'xx!'[:-1]: 3}) == (3, 2)

import random
from pathlib import Path

random.seed('abc!'[:-1])
first = random.random()
random.seed('abc')
assert random.random() == first
assert str(Path('abc!'[:-1])) == 'abc'
assert json.loads('{"abc": 1, "abc": 2}') == {'abc': 2}
