use monty::{Dump, MontyRepl, MontyRun, Session, SessionRef, dump};
use monty_types::{CompileOptions, PrintWriter, ResourceTracker};

#[test]
fn bytearray_review_regressions() {
    for source in [
        include_str!("../test_cases/bytearray__buffers.py"),
        include_str!("../test_cases/bytearray__empty_slices.py"),
        include_str!("../test_cases/bytearray__result_copies.py"),
        include_str!("../test_cases/bytearray__replace_callbacks.py"),
        include_str!("../test_cases/bytes__search_integer.py"),
        include_str!("../test_cases/bytes__prefix_suffix_buffers.py"),
        include_str!("../test_cases/bytes__ord_reversed.py"),
        include_str!("../test_cases/bytes__hex_optional.py"),
    ] {
        let mut runner = MontyRun::new(source.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
        runner.run_no_limits(vec![]).unwrap();
    }
}

#[test]
fn bytearray_core() {
    let source = include_str!("../test_cases/bytearray__core.py");
    let mut runner = MontyRun::new(source.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    runner.run_no_limits(vec![]).unwrap();
}

#[test]
fn bytearray_parity() {
    let source = include_str!("../test_cases/bytearray__parity.py");
    let mut runner = MontyRun::new(source.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    runner.run_no_limits(vec![]).unwrap();
}

#[test]
fn bytearray_snapshot_retains_aliases_and_iterator_position() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "a = bytearray(b'abc')\nb = a\nit = iter(a)\nassert next(it) == 97",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();
    let bytes = dump("test.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else {
        panic!("expected idle snapshot");
    };
    restored
        .feed_run(
            "a[1] = 90\nassert b is a\nassert next(it) == 90\nassert list(it) == [99]\na.append(1)\nassert list(it) == []",
            vec![],
            PrintWriter::Stdout,
        )
        .unwrap();
}

#[test]
fn bytearray_inplace() {
    let source = include_str!("../test_cases/bytearray__inplace.py");
    let mut runner = MontyRun::new(source.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    runner.run_no_limits(vec![]).unwrap();
}

#[test]
fn bytearray_join_exports() {
    let source = include_str!("../test_cases/bytearray__join_exports.py");
    let mut runner = MontyRun::new(source.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    runner.run_no_limits(vec![]).unwrap();
}

#[test]
fn bytearray_search_exports() {
    // Adapted from CPython's gh-142560 search-callback regression.
    let source = r"
for method in ['find', 'rfind', 'count', 'index', 'rindex']:
    data = bytearray(b'abc')

    class Needle:
        def __index__(self):
            data.clear()
            return 97

    try:
        if method == 'find':
            data.find(Needle())
        elif method == 'rfind':
            data.rfind(Needle(), -3)
        elif method == 'count':
            data.count(Needle(), None, None)
        elif method == 'index':
            data.index(Needle(), 100, 0)
        else:
            data.rindex(Needle())
    except BufferError as exc:
        assert str(exc) == 'Existing exports of data: object cannot be re-sized'
    else:
        assert False, method
    assert data == b'abc'
    data.append(33)
    assert data == b'abc!'
";
    let mut runner = MontyRun::new(source.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    runner.run_no_limits(vec![]).unwrap();
}
