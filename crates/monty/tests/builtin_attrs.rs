use monty::MontyRun;
use monty_types::{CompileOptions, MontyObject};

#[test]
fn table_methods_borrow_receivers_on_success_and_error() {
    let code = r"
from datetime import date, time, timedelta, timezone
import re

items = [2, 1]
items.append(3)
items.sort()
assert items.count(3) == 1
assert (1, 2).index(2) == 1
assert complex(1, 2).conjugate() == complex(1, -2)
assert date(2024, 1, 2).isoformat() == '2024-01-02'
assert time(1, 2, 3).isoformat() == '01:02:03'
assert timedelta(seconds=2).total_seconds() == 2.0
assert timezone.utc.utcoffset(None) == timedelta(0)

pattern = re.compile('(a)')
match = pattern.search('a')
assert match.group(1) == 'a'
assert pattern.sub('b', 'a') == 'b'
assert re.compile('a').split('a') == ['', '']

try:
    items.append()
except TypeError:
    pass
else:
    raise AssertionError('append should require an argument')
try:
    pattern.search(1)
except TypeError:
    pass
else:
    raise AssertionError('search should require a string')
assert pattern.search('a').group() == 'a'
items
";
    let mut run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(
        run.run_no_limits(vec![]).unwrap(),
        MontyObject::list([MontyObject::int(1), MontyObject::int(2), MontyObject::int(3)])
    );
}

#[test]
fn class_methods_dispatch_through_attribute_tables() {
    let code = r"
from datetime import date, time

assert date.fromisoformat('2024-01-02') == date(2024, 1, 2)
assert time.fromisoformat('01:02:03') == time(1, 2, 3)
assert complex.from_number(2) == complex(2)
try:
    date.today(1)
except TypeError:
    pass
else:
    raise AssertionError('today should reject arguments')
True
";
    let mut run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(run.run_no_limits(vec![]).unwrap(), MontyObject::bool(true));
}
