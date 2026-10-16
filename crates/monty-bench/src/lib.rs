//! Shared graph fixtures for execution timings and allocator measurements.

use monty::{Dump, MontyRepl, MontyRun, RunProgress, Session, SessionRef, dump};
use monty_types::{CompileOptions, MontyObject, NameLookupResult, PrintWriter, ResourceTracker};

/// Builds allocated strings, shared containers, cycles and identity-hashed keys.
///
/// # Panics
/// Panics if the fixture cannot execute.
#[must_use]
pub fn heap_graph_session() -> MontyRepl {
    let mut repl = MontyRepl::new("bench.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "class Key: pass\n\
         key = Key()\n\
         items = [[i, str(i)] for i in range(5_000)]\n\
         aliases = [items, items]\n\
         items.append(aliases)\n\
         mapping = {key: items, (Key, key): aliases}\n\
         saved_id = id(items)\n\
         saved_hash = hash(key)",
        vec![],
        PrintWriter::Disabled,
    )
    .unwrap();
    repl
}

/// Includes load, compilation, execution, dump and session destruction in one checkpoint.
///
/// # Panics
/// Panics unless `bytes` is an idle session dump and `code` executes successfully.
#[must_use]
pub fn heap_checkpoint(bytes: &[u8], code: &str) -> (Vec<u8>, MontyObject) {
    let Session::Idle(mut repl) = Dump::load(bytes).unwrap().state else {
        panic!("expected idle graph session")
    };
    let result = repl.feed_run(code, vec![], PrintWriter::Disabled).unwrap();
    let bytes = dump("bench.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    (bytes, result)
}

/// Compiles a missing-name suspension while retaining a list in VM globals.
///
/// # Panics
/// Panics if the fixture cannot compile.
#[must_use]
pub fn heap_suspension_program() -> MontyRun {
    MontyRun::new(
        "root = [42]\nmissing\nroot[0]".to_owned(),
        "bench.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap()
}

/// Prices parking and restoring an owner without serialization; compilation is excluded.
///
/// # Panics
/// Panics unless `run` suspends once on a missing name, then completes when it receives None.
#[must_use]
pub fn heap_suspend_resume(run: &MontyRun) -> MontyObject {
    let RunProgress::NameLookup(lookup) = run
        .clone()
        .start(vec![], ResourceTracker::default(), PrintWriter::Disabled)
        .unwrap()
    else {
        panic!("expected name lookup")
    };
    let RunProgress::Complete(value) = lookup
        .resume(NameLookupResult::Value(MontyObject::none()), PrintWriter::Disabled)
        .unwrap()
    else {
        panic!("expected completion")
    };
    value
}
