//! Function identity, aliases and persisted container keys survive session dumps.

use monty::{Dump, MontyRepl, Session, SessionRef, dump};
use monty_types::{CompileOptions, PrintWriter, ResourceTracker};

#[test]
fn distinct_functions_and_container_keys_round_trip() {
    let mut repl = MontyRepl::new("functions.py", ResourceTracker::default(), CompileOptions::default());
    run(&mut repl, include_str!("../test_cases/function__identity.py"));
    run(&mut repl, "saved_hashes = [hash(item[0]) for item in examples]");
    let bytes = dump("functions.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let mut restored = restore(&bytes);
    run(
        &mut restored,
        r"
for index, (first, second, alias, mapping) in enumerate(examples):
    assert first is alias
    assert first != second
    assert hash(first) == saved_hashes[index]
    assert len({first, second, alias}) == 2
    assert mapping[alias] == 'first'
    assert mapping[second] == 'second'
",
    );
}

/// Written by the unmodified v15 worker: keep its inline function aliases and
/// code-based hashes usable, while subsequent definitions get fresh objects.
#[test]
fn version_15_function_aliases_and_keys_remain_usable() {
    let mut repl = restore(include_bytes!("fixtures/functions_v15.bin"));
    let checks = "assert plain is aliases[0]\nassert defaulted is aliases[1]\n\
                  assert closed is aliases[2]\nassert len(members) == 3\n\
                  assert mapping[plain] == 1\nassert mapping[defaulted] == 2\n\
                  assert mapping[closed] == 3\n\
                  assert hashes == [hash(plain), hash(defaulted), hash(closed)]\n\
                  assert plain() == 1\nassert defaulted() == 2\nassert closed() == 3";
    run(&mut repl, checks);
    run(
        &mut repl,
        "fresh = plain_factory()\nassert fresh is not plain\nassert fresh != plain\n\
         assert default_factory() != defaulted\nassert closure_factory(3) != closed\n\
         mapping[fresh] = 4\nassert len(mapping) == 4",
    );
    let bytes = dump("functions.py", None, &[], SessionRef::Idle(&repl)).unwrap();
    let mut restored = restore(&bytes);
    run(&mut restored, checks);
    run(&mut restored, "assert mapping[fresh] == 4\nassert fresh != plain");
}

fn run(repl: &mut MontyRepl, code: &str) {
    repl.feed_run(code, vec![], PrintWriter::Stdout).unwrap();
}

fn restore(bytes: &[u8]) -> MontyRepl {
    match Dump::load(bytes).unwrap().state {
        Session::Idle(repl) => *repl,
        _ => panic!("expected an idle session"),
    }
}
