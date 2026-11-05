use monty::{Dump, MontyRepl, MontyRun, Session, SessionRef, dump};
#[cfg(feature = "ref-count-return")]
use monty_types::ResourceLimits;
use monty_types::{CompileOptions, MontyObject, PrintWriter, ResourceTracker};

#[test]
fn string_views_preserve_python_behavior() {
    let code = r"
s = (' a🌍éZ ' * 200)
v = s[10:1000]
expected = ''.join(list(s)[10:1000])
assert v == expected
assert len(v) == len(expected)
assert hash(v) == hash(expected)
assert {v: 42}[expected] == 42
assert v.strip() == expected.strip()
assert v + '!' == expected + '!'
assert '{}'.format(v) == expected
assert v[::-1] == ''.join(list(expected)[::-1])
assert v[::2] == ''.join(list(expected)[::2])
assert v[:] is v
assert v[::1] is v
assert v[100:10] == ''
assert v[-1000:10000] is v
s = None
assert v == expected
v = v[10:400][20:300]
assert v == expected[10:400][20:300]
assert v[1] == expected[31]
for step in [-3, -1, 1, 2, 3]:
    for start in [-10000, -200, -1, 0, 1, 200, 10000]:
        for stop in [-10000, -200, -1, 0, 1, 200, 10000]:
            assert v[start:stop:step] == ''.join(list(v)[start:stop:step])
True
";
    let mut runner = MontyRun::new(code.to_owned(), "views.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(runner.run_no_limits(vec![]).unwrap(), MontyObject::bool(true));
}

#[test]
fn snapshot_restores_views_after_source_is_released() {
    let mut repl = MontyRepl::new("views.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "source = 'é🌍' * 2000\nview = source[100:2000]\nsource = None\nlookup = {view: 7}\nit = iter(view)\nassert next(it) == 'é'",
        vec![],
        PrintWriter::Disabled,
    ).unwrap();
    let bytes = dump("views.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else {
        panic!("expected idle session");
    };
    let result = restored.feed_run(
        "assert view == ('é🌍' * 950)\nassert lookup[view] == 7\nassert next(it) == '🌍'\nassert view[10:500] == ('é🌍' * 245)\nTrue",
        vec![],
        PrintWriter::Disabled,
    ).unwrap();
    assert_eq!(result, MontyObject::bool(true));
}

#[test]
fn runners_remain_send_with_shared_strings() {
    fn assert_send<T: Send>() {}
    assert_send::<MontyRun>();
    assert_send::<MontyRepl>();
}

#[test]
fn json_strings_survive_decoder_scratch_reuse_and_hooks() {
    let code = r#"
import json
seen = []
def parse_number(value):
    seen.append('temporary' * 100)
    return int(value)
value = json.loads(r'{"escaped-key-\u00e9-0001": "first-escaped-value-\u00fc-0001", "escaped-key-\u00e9-0002": "second-escaped-value-\u00fc-0002", "number": 123}', parse_int=parse_number)
assert value['escaped-key-é-0001'] == 'first-escaped-value-ü-0001'
assert value['escaped-key-é-0002'] == 'second-escaped-value-ü-0002'
assert value['number'] == 123
assert len(seen) == 1
encoded = '[' + ','.join(['"escaped-\\u00e9-' + str(i) + '-' + 'x' * 200 + '"' for i in range(100)]) + ']'
values = json.loads(encoded)
for i in range(100):
    assert values[i] == 'escaped-é-' + str(i) + '-' + 'x' * 200
True
"#;
    let mut runner = MontyRun::new(code.to_owned(), "json-scratch.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(runner.run_no_limits(vec![]).unwrap(), MontyObject::bool(true));
}

#[test]
fn splitline_views_preserve_boundaries_and_outlive_source_views() {
    let code = r"
payload = 'é🌍' + 'x' * 28
for ending in ['\n', '\r', '\r\n', '\v', '\f', '\x1c', '\x1d', '\x1e', '\x85', '\u2028', '\u2029']:
    body = ending.join([payload] * 100) + ending
    source = 'head' * 100 + body + 'tail' * 100
    window = source[400:-400]
    assert window.splitlines() == [payload] * 100
    assert window.splitlines(True) == [payload + ending] * 100
    line = window.splitlines()[57]
    source = None
    window = None
    body = None
    assert line == payload
    assert line[0] == 'é'
    assert line[1] == '🌍'
    assert hash(line) == hash(payload)
assert ('x' * 20 + '\n\nshort\n').splitlines() == ['x' * 20, '', 'short']
True
";
    let mut runner = MontyRun::new(code.to_owned(), "splitline-views.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(runner.run_no_limits(vec![]).unwrap(), MontyObject::bool(true));
}

#[test]
fn repeated_unicode_trims_keep_character_boundaries() {
    let code = r"
source = 'aé中🐍' * 500
for left in range(9):
    for right in range(9):
        view = source[100:1900]
        expected = list(source)[100:1900]
        for iteration in range(12):
            end = len(view) - right
            view = view[left:end]
            expected = expected[left:len(expected) - right]
            assert view == ''.join(expected)
            assert len(view) == len(expected)
        assert view[:] is view
source = None
assert view == ''.join(expected)
True
";
    let mut runner = MontyRun::new(code.to_owned(), "unicode-trims.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(runner.run_no_limits(vec![]).unwrap(), MontyObject::bool(true));
}

#[test]
fn many_view_hashes_survive_source_release_and_snapshot() {
    let mut repl = MontyRepl::new("hashes.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "source = 'aé中🐍' * 4096\nviews = [source[i:i + 8192] for i in range(64)]\nhashes = [hash(v) for v in views]\nlookup = {v: len(v) for v in views}\nsource = None\nfor repeat in range(3):\n    assert [hash(v) for v in views] == hashes",
        vec![], PrintWriter::Disabled,
    ).unwrap();
    let bytes = dump("hashes.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else {
        panic!("expected idle session");
    };
    assert_eq!(
        restored
            .feed_run(
                "assert [hash(v) for v in views] == hashes\nassert all(lookup[v] == len(v) for v in views)\nTrue",
                vec![],
                PrintWriter::Disabled,
            )
            .unwrap(),
        MontyObject::bool(true)
    );
}

#[test]
#[cfg(feature = "ref-count-return")]
fn inline_views_share_one_heap_entry_and_remain_reachable() {
    for count in [1, 1000] {
        let code = format!(
            "source = 'é中🐍a' * 10000\nviews = [source[i:i + 20000] for i in range({count})]\nsource = None\nviews"
        );
        let mut runner = MontyRun::new(code, "view-refs.py", vec![], CompileOptions::default()).unwrap();
        let output = runner.run_ref_counts(vec![]).unwrap();
        assert_eq!(
            output.heap_count, 2,
            "only the result list and original backing need heap entries"
        );
        assert!(output.unreachable.is_empty(), "{:?}", output.unreachable);
    }
}

#[test]
#[cfg(feature = "ref-count-return")]
fn cycles_containing_views_release_their_backings() {
    let code = r"
for i in range(200):
    source = ('é中🐍a' + str(i)) * 1000
    view = source[10:2000]
    holder = [view]
    cycle = {'view': view, 'holder': holder}
    holder.append(cycle)
source = None
view = None
holder = None
cycle = None
for i in range(20):
    garbage = [i]
True
";
    let tracker = ResourceTracker::new(ResourceLimits::default().gc_interval(1));
    let mut runner = MontyRun::new(code.to_owned(), "view-cycles.py", vec![], CompileOptions::default()).unwrap();
    let output = runner.run_ref_counts_with_tracker(vec![], tracker).unwrap();
    assert_eq!(output.value, MontyObject::bool(true));
    assert!(output.unreachable.is_empty(), "{:?}", output.unreachable);
    assert!(
        output.heap_count <= 2,
        "backings retained by collected cycles: {}",
        output.heap_count
    );
}
