//! Module functions retained in session state survive a dump/load cycle.

use monty::{Dump, MontyRepl, Session, SessionRef, dump};
use monty_types::{CompileOptions, PrintWriter, ResourceTracker};

#[test]
fn bisect_functions_survive_a_dump() {
    let mut repl = MontyRepl::new("bisect.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "from bisect import bisect_left, bisect_right, insort_left, insort_right\na = [(1, 'a'), (2, 'b')]",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();
    let bytes = dump("bisect.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else {
        panic!("expected an idle session");
    };
    restored
        .feed_run(
            "\
assert bisect_left(a, 2, key=lambda x: x[0]) == 1
assert bisect_right(a, 2, key=lambda x: x[0]) == 2
insort_left(a, (2, 'left'), key=lambda x: x[0])
insort_right(a, (2, 'right'), key=lambda x: x[0])
assert a == [(1, 'a'), (2, 'left'), (2, 'b'), (2, 'right')]
",
            vec![],
            PrintWriter::Stdout,
        )
        .unwrap();
}
