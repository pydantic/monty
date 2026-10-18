use monty::{Dump, MontyRepl, Session, SessionRef, dump};
use monty_types::{CompileOptions, MontyObject, PrintWriter, ResourceTracker};

#[test]
fn numeric_and_promoted_lists_survive_snapshots() {
    let mut repl = MontyRepl::new("lists.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "empty = []\ncycle = []\ncycle.append(cycle)\nints = [1, 2, 3]\nfloats = [1.5, -0.0]\nmixed = [1, 2]\nmixed.append(None)\nit = iter(ints)\nnext(it)",
        vec![],
        PrintWriter::Disabled,
    )
    .unwrap();
    let bytes = dump("lists.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let loaded = Dump::load(&bytes).unwrap();
    let Session::Idle(mut restored) = loaded.state else {
        panic!("idle session expected")
    };
    let output = restored.feed_run(
        "assert cycle[0] is cycle\nassert empty == []\nempty.append(1)\nassert sum(empty) == 1\nassert list(it) == [2, 3]\nassert floats == [1.5, -0.0]\nassert str(floats[1]) == '-0.0'\nassert mixed == [1, 2, None]\nints.append(True)\nassert type(ints[-1]) is bool\nints.clear()\nints.append(2.5)\nassert type(ints[0]) is float\nsum(mixed[:2])",
        vec![],
        PrintWriter::Disabled,
    ).unwrap();
    assert_eq!(output, MontyObject::int(3));
}
