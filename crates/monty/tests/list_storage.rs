use monty::{Dump, MontyRepl, Session, SessionRef, dump};
use monty_types::{CompileOptions, ExcType, MontyObject, PrintWriter, ResourceLimits, ResourceTracker};

#[test]
fn failed_integer_list_widening_preserves_the_list_and_iterator() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "items = [1] * 20_000\nalias = items\nit = iter(items)\nnext(it)",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();

    // The bitmap fits the budget. A replacement Value buffer would exceed it.
    *repl.tracker_mut() = ResourceTracker::new(ResourceLimits::default().max_memory(200_000));
    repl.feed_run("items[1] = None\nitems[-1] = None", vec![], PrintWriter::Stdout)
        .unwrap();
    let error = repl
        .feed_run("items.append(True)", vec![], PrintWriter::Stdout)
        .unwrap_err();
    assert_eq!(error.exc_type(), ExcType::MemoryError);

    *repl.tracker_mut() = ResourceTracker::default();
    let result = repl
        .feed_run(
            "alias is items and len(items) == 20_000 and next(it) is None and items[-1] is None",
            vec![],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(result, MontyObject::bool(true));
}

#[test]
fn packed_lists_and_iterators_round_trip_through_the_existing_dump_format() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "items = [1, None, 0, 256]\nsmall = [0, 255]\nnullable_small = [255, None, 0]\nflags = [True, None, False]\nempty_slots = [None] * 64\nalias = items\nit = iter(items)\nnext(it)",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();

    let bytes = dump("test.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else {
        panic!("expected an idle session");
    };
    let result = restored
        .feed_run(
            "items.append(True)\nalias is items and list(it) == [None, 0, 256, True] and small == [0, 255] and nullable_small == [255, None, 0] and type(items[2]) is int and items[-1] is True and flags[0] is True and flags[1] is None and flags[2] is False and empty_slots[-1] is None",
            vec![],
            PrintWriter::Stdout,
        )
        .unwrap();
    assert_eq!(result, MontyObject::bool(true));
}
