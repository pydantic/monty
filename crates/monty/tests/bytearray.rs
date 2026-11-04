use monty::{Dump, MontyRepl, MontyRun, Session, SessionRef, dump};
use monty_types::{CompileOptions, PrintWriter, ResourceTracker};

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
