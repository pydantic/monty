//! Shared string views must survive session dumps and parent release.

use std::fmt::Write;

use monty::{Dump, MontyRepl, Session, SessionRef, dump};
use monty_types::{CompileOptions, ExcType, MontyObject, PrintWriter, ResourceLimits, ResourceTracker};

#[test]
fn append_handles_wide_local_stores() {
    let mut source = String::from("def build():\n");
    for index in 0..270 {
        writeln!(source, "    value_{index} = {index}").unwrap();
    }
    source.push_str(
        "    text = 'long starting value'\n    for i in range(1000):\n        text += 'x'\n    return text\nbuild()",
    );
    let mut repl = MontyRepl::new("append.py", ResourceTracker::default(), CompileOptions::default());
    assert_eq!(
        repl.feed_run(&source, vec![], PrintWriter::Disabled).unwrap(),
        MontyObject::string(format!("long starting value{}", "x".repeat(1000))),
    );
}

#[test]
fn append_growth_rejection_preserves_target() {
    let mut repl = MontyRepl::new("append.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run("text = 'x' * 80\nsuffix = 'tail'", vec![], PrintWriter::Disabled)
        .unwrap();
    *repl.tracker_mut() = ResourceTracker::new(ResourceLimits::default().max_memory(0));
    let error = repl
        .feed_run("text += suffix", vec![], PrintWriter::Disabled)
        .unwrap_err();
    assert_eq!(error.exc_type(), ExcType::MemoryError);
    assert_eq!(error.message(), Some("memory limit exceeded: 80 bytes > 0 bytes"));
    *repl.tracker_mut() = ResourceTracker::default();
    assert_eq!(
        repl.feed_run("text", vec![], PrintWriter::Disabled).unwrap(),
        MontyObject::string("x".repeat(80)),
    );
}

#[test]
fn shared_addition_rejection_preserves_aliases() {
    let mut repl = MontyRepl::new("shared.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "text = 'x' * 80\nalias = text\nsuffix = 'tail'",
        vec![],
        PrintWriter::Disabled,
    )
    .unwrap();
    *repl.tracker_mut() = ResourceTracker::new(ResourceLimits::default().max_memory(0));
    let error = repl
        .feed_run("text + suffix", vec![], PrintWriter::Disabled)
        .unwrap_err();
    assert_eq!(error.exc_type(), ExcType::MemoryError);
    assert_eq!(error.message(), Some("memory limit exceeded: 80 bytes > 0 bytes"));
    *repl.tracker_mut() = ResourceTracker::default();
    assert_eq!(
        repl.feed_run("assert text is alias\ntext == 'x' * 80", vec![], PrintWriter::Disabled)
            .unwrap(),
        MontyObject::bool(true),
    );
}

#[test]
fn append_after_dump_preserves_snapshots_and_views() {
    let mut repl = MontyRepl::new("append.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "text = 'x' * 80\nfor i in range(500):\n    text += 'é'\nsnapshot = text\nview = text[10:]\ntext += 'tail'",
        vec![],
        PrintWriter::Disabled,
    )
    .unwrap();
    let bytes = dump("append.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else {
        panic!("expected idle session")
    };
    for session in [&mut repl, &mut restored] {
        let result = session
            .feed_run(
                "text += 'end'\nassert snapshot == 'x' * 80 + 'é' * 500\nassert view == snapshot[10:]\ntext == snapshot + 'tailend'",
                vec![],
                PrintWriter::Disabled,
            )
            .unwrap();
        assert_eq!(result, MontyObject::bool(true));
    }
}

#[test]
fn shared_views_survive_session_dump_and_parent_release() {
    let mut repl = MontyRepl::new("views.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "source = 'abcde' * 1000\na = source[100:4900]\nb = a[100:4700]\nlookup = {a: b}\nsource = None",
        vec![],
        PrintWriter::Disabled,
    )
    .unwrap();
    let bytes = dump("views.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else {
        panic!("expected idle session")
    };
    for session in [&mut repl, &mut restored] {
        let result = session
            .feed_run(
                "assert lookup[a] == b\na = None\nlookup = None\nb == ('abcde' * 1000)[200:4800]",
                vec![],
                PrintWriter::Disabled,
            )
            .unwrap();
        assert_eq!(result, MontyObject::bool(true));
    }
}
