#[cfg(feature = "test-hooks")]
use std::time::Instant;
use std::{fmt::Write, iter::repeat_n, thread::Builder, time::Duration};

use monty::{MontyRepl, MontyRun};
use monty_types::{CompileOptions, ExcType, MontyObject, PrintWriter, ResourceLimits, ResourceTracker};

fn check(source: &str) {
    let mut runner = MontyRun::new(source.to_owned(), "dispatch.py", vec![], CompileOptions::default()).unwrap();
    runner.run_no_limits(vec![]).unwrap();
    #[cfg(feature = "ref-count-return")]
    {
        let result = runner.run_ref_counts(vec![]).unwrap();
        assert!(result.unreachable.is_empty(), "{:?}", result.unreachable);
    }
    #[cfg(not(feature = "ref-count-return"))]
    runner.run_no_limits(vec![]).unwrap();
}

#[test]
fn local_runs_preserve_frame_changes_errors_and_reference_ownership() {
    check(
        r"
def copy(value):
    alias = value
    a = alias
    b = a
    a = b
    return a

def recurse(n, value):
    if n:
        return recurse(n - 1, copy(value))
    return copy(value)

for i in range(400):
    value = [i]
    assert recurse(4, value) is value

def unbound():
    a = 1
    b = a
    try:
        b = missing
    except UnboundLocalError:
        return b
    missing = 2
assert unbound() == 1

try:
    recurse(1, [1])[5]
except IndexError:
    pass
else:
    assert False

def large():
    value = 1234567890123456789012345678901234567890
    alias = value
    return alias
assert large() == 1234567890123456789012345678901234567890

",
    );
}

#[test]
fn wide_locals_and_checkpoint_long_runs_preserve_values() {
    let mut source = "def wide():\n".to_owned();
    for slot in 0..300 {
        writeln!(source, "    a{slot} = [{slot}]").unwrap();
    }
    for _ in 0..3 {
        for slot in 0..300 {
            writeln!(source, "    a{slot} = a{slot}").unwrap();
        }
    }
    source.push_str("    return a299\nassert wide() == [299]\n");
    check(&source);
}

#[test]
fn iteration_stores_preserve_wide_slots_and_reference_replacement() {
    let mut source = "def wide():\n".to_owned();
    for slot in 0..300 {
        writeln!(source, "    a{slot} = {slot}").unwrap();
    }
    source.push_str(
        "    shared = []\n    for a299 in range(1000):\n        pass\n    assert a299 == 999\n    for a299 in [shared, 1, shared, None] * 100:\n        if isinstance(a299, list):\n            assert a299 is shared\n    assert a299 is None\nwide()\n",
    );
    check(&source);
}

#[test]
fn direct_arithmetic_preserves_boundaries_and_fallbacks() {
    check(
        r"
smallest = -9223372036854775808
largest = 9223372036854775807
assert -smallest == 9223372036854775808
assert largest << 1 == 18446744073709551614
assert -1 << 64 == -18446744073709551616
assert smallest >> 100 == -1
assert largest >> 100 == 0
assert ~smallest == largest
assert ~largest == smallest
assert (True & False) is False
assert (True | False) is True
assert (True ^ True) is False
assert type(+True) is int
assert not -0.0
assert (not float('nan')) is False
assert -3 // 2 == -2
assert -3 % 2 == 1
for code in ['1 << -1', '1 >> -1', '1 / 0', '1 // 0', '1 % 0']:
    try:
        eval(code)
    except (ValueError, ZeroDivisionError):
        pass
    else:
        assert False
class Number:
    def __and__(self, other):
        return 42
    def __neg__(self):
        return 43
number = Number()
assert number & 1 == 42
assert -number == 43
",
    );
}

#[cfg(feature = "test-hooks")]
#[test]
fn native_runs_keep_deadline_checkpoints() {
    let mut runner = MontyRun::opcode_cost_benchmark(10_000_000);
    let tracker = ResourceTracker::new(ResourceLimits::default().max_feed_duration(Duration::from_millis(1)));
    let start = Instant::now();
    let error = runner.run(vec![], tracker, PrintWriter::Disabled).unwrap_err();
    assert_eq!(error.exc_type(), ExcType::TimeoutError);
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn threaded_subscription_keeps_callbacks_mutations_and_caught_errors() {
    check(
        r"
def read(d):
    value = d['x']
    return value

for i in range(300):
    a = {'x': [i], 'y': [2]}
    b = {'y': [3], 'x': [i + 1]}
    assert read(a) is a['x']
    assert read(b) is b['x']
    a.pop('x')
    a['x'] = [i + 2]
    assert read(a) == [i + 2]
    a.clear()
    try:
        read(a)
    except KeyError:
        pass
    else:
        assert False

calls = []
class Key:
    def __hash__(self):
        return hash('x')
    def __eq__(self, other):
        calls.append(other)
        return other == 'x'
k = Key()
d = {k: [5]}
assert read(d) == [5]
assert len(calls) > 0

class Record:
    pass
try:
    read(Record())
except TypeError:
    pass
else:
    assert False

assert [1, 2][1] == 2
assert (3, 4)[1] == 4
assert 'abc'[1] == 'b'
from collections import defaultdict, Counter
missing = defaultdict(list)
assert missing['new'] == []
assert missing['new'] is missing['new']
assert Counter()['missing'] == 0

try:
    read({})
except KeyError:
    assert read({'x': [9]}) == [9]
else:
    assert False
",
    );
}

#[test]
fn threaded_subscription_preserves_traceback_and_task_errors() {
    let source = "def read(d):\n    value = d['missing']\n    return value\nread({})";
    let mut runner = MontyRun::new(source.to_owned(), "subscript.py", vec![], CompileOptions::default()).unwrap();
    let error = runner.run_no_limits(vec![]).unwrap_err();
    assert_eq!(error.exc_type(), ExcType::KeyError);
    let frame = error.traceback().last().unwrap();
    assert_eq!(frame.frame_name.as_deref(), Some("read"));
    assert_eq!(frame.start.line, 2);

    check(
        r"
import asyncio
async def caught(i):
    try:
        value = {}['missing']
    except KeyError:
        return i
assert await asyncio.gather(caught(1), caught(2)) == [1, 2]

async def failed():
    return {}['missing']
async def sibling():
    return 7
try:
    await asyncio.gather(failed(), sibling())
except KeyError:
    assert {'x': [3]}['x'] == [3]
else:
    assert False
",
    );
}

#[test]
fn threaded_subscription_observes_deadlines_and_recovers() {
    let tracker = ResourceTracker::new(ResourceLimits::default().max_feed_duration(Duration::from_millis(20)));
    let mut repl = MontyRepl::new("deadline.py", tracker, CompileOptions::default());
    let error = repl
        .feed_run(
            "def hot(d):\n    while True:\n        value = d['x']\nhot({'x': [1]})",
            vec![],
            PrintWriter::Disabled,
        )
        .unwrap_err();
    assert_eq!(error.exc_type(), ExcType::TimeoutError);
    assert_eq!(
        repl.feed_run("1 + 1", vec![], PrintWriter::Disabled).unwrap(),
        MontyObject::int(2)
    );
}

#[test]
fn nested_dispatch_batches_observe_deadlines_and_recover() {
    let tracker = ResourceTracker::new(ResourceLimits::default().max_feed_duration(Duration::from_millis(20)));
    let mut repl = MontyRepl::new("nested_deadline.py", tracker, CompileOptions::default());
    let error = repl
        .feed_run(
            "class Slow:\n    def __getitem__(self, index):\n        while True:\n            pass\nSlow()[0]",
            vec![],
            PrintWriter::Disabled,
        )
        .unwrap_err();
    assert_eq!(error.exc_type(), ExcType::TimeoutError);
    assert_eq!(
        repl.feed_run("1 + 1", vec![], PrintWriter::Disabled).unwrap(),
        MontyObject::int(2)
    );
}

#[cfg(feature = "test-hooks")]
#[test]
fn native_batches_end_with_the_same_stack_and_counter_state() {
    for count in [254, 255, 256, 257, 511, 512, 513, 100_000] {
        let mut runner = MontyRun::opcode_cost_benchmark(count);
        assert_eq!(runner.run_no_limits(vec![]).unwrap(), MontyObject::int(0));
    }
}

#[test]
fn callback_reentry_with_long_dispatch_batches_fits_small_stack() {
    Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            let mut source = "class Recursive:\n    def __eq__(self, other):\n        a = 0\n".to_owned();
            for _ in 0..120 {
                source.push_str("        a = a\n");
            }
            source.push_str("        return self == other\ntry:\n    Recursive() == Recursive()\nexcept RecursionError:\n    pass\nelse:\n    assert False\n");
            check(&source);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn direct_branches_preserve_short_circuit_values_and_heap_truth() {
    check(
        r"
class Marker:
    pass
marker = [9]
for _ in range(300):
    for value in [None, False, True, 0, 1, -1, 0.0, 1.0, [], [1], '', 'x', Marker(), 123456789012345678901234567890]:
        truth = bool(value)
        if truth:
            assert (value or marker) is value
            assert (value and marker) is marker
        else:
            assert (value or marker) is marker
            assert (value and marker) is value
",
    );
}

#[test]
fn cloned_operands_preserve_aliases_when_the_stack_grows() {
    let values = repeat_n("value", 300).collect::<Vec<_>>().join(", ");
    let constants = (0..300).map(|i| i.to_string()).collect::<Vec<_>>().join(", ");
    let source = format!(
        "def grow(value):\n    return ({values})\nvalue = []\nitems = grow(value)\nassert len(items) == 300\nfor item in items:\n    assert item is value\nconstants = ({constants})\nassert constants[299] == 299\nitems = None\nvalue = None\nitem = None\n"
    );
    check(&source);
}

#[test]
fn typed_iteration_observes_mutation_aliases_and_range_endpoints() {
    check(
        r"
values = [[1], [2]]
seen = []
for value in values:
    seen.append(value)
    if len(seen) == 1:
        values.append([3])
assert seen == [[1], [2], [3]]
assert seen[2] is values[2]

for value in values:
    values.clear()
assert values == []

value = []
for item in (value, value):
    assert item is value

seen = []
for value in range(9223372036854775805, 9223372036854775807):
    seen.append(value)
assert seen == [9223372036854775805, 9223372036854775806]
seen = []
for value in range(-9223372036854775806, -9223372036854775808, -1):
    seen.append(value)
assert seen == [-9223372036854775806, -9223372036854775807]

for value in range(0):
    assert False
",
    );
}
