//! `hashlib` behaviour that only a Rust test can see: a hash mid-stream is
//! session state, so it survives a dump and finishes to the same digest.
//! Everything Python-visible is covered by `test_cases/hashlib__*.py`.

use monty::{Dump, MontyRepl, Session, SessionRef, dump};
use monty_types::{CompileOptions, MontyObject, PrintWriter, ResourceTracker};

/// Runs `code` as one feed of a fresh session.
fn run(code: &str) -> MontyObject {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(code, vec![], PrintWriter::Stdout).unwrap()
}

#[test]
fn a_hash_mid_stream_survives_a_dump() {
    for (constructor, digest) in [
        ("hashlib.sha256()", "hexdigest()"),
        ("hashlib.sha3_512()", "hexdigest()"),
        ("hashlib.shake_128()", "hexdigest(16)"),
        ("hashlib.blake2b(key=b'k')", "hexdigest()"),
    ] {
        let start = format!("import hashlib\nh = {constructor}\nh.update(b'ab' * 100)");
        let finish = format!("h.update(b'c' * 200)\nh.{digest}");
        let one_shot = run(&format!("{start}\n{finish}"));

        let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
        repl.feed_run(&start, vec![], PrintWriter::Stdout).unwrap();
        let bytes = dump("test.py", None, SessionRef::Idle(&repl)).unwrap();
        let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else {
            panic!("expected an idle session");
        };
        let after_dump = restored.feed_run(&finish, vec![], PrintWriter::Stdout).unwrap();
        assert_eq!(after_dump, one_shot, "{constructor}");
    }
}
