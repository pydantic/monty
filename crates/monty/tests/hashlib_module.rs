//! `hashlib` behaviour that only a Rust test can see: a hash mid-stream is
//! session state, so it survives a dump and finishes to the same digest.
//! Everything Python-visible is covered by `test_cases/hashlib__*.py`.

use std::fmt::Write;

use insta::assert_snapshot;
use monty::{Dump, MontyRepl, Session, SessionRef, dump};
use monty_types::{CompileOptions, MontyObject, PrintWriter, ResourceTracker};
use sha2::digest::{Digest, common::hazmat::SerializableState};

/// Runs `code` as one feed of a fresh session.
fn run(code: &str) -> MontyObject {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(code, vec![], PrintWriter::Stdout).unwrap()
}

/// Runs `code`, which must raise, and returns the exception as `Type: message`.
fn run_err(code: &str) -> String {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    let exc = repl.feed_run(code, vec![], PrintWriter::Stdout).unwrap_err();
    format!("{}: {}", exc.exc_type(), exc.message().unwrap_or_default())
}

/// `hashlib.shake_128` is OpenSSL-backed in CPython, so the `_sha3` wording
/// Monty uses for a bad length cannot be pinned by a dual-run case.
#[test]
fn shake_length_errors_use_the_sha3_wording() {
    for (call, expected) in [
        ("digest(-1)", "ValueError: Cannot convert negative int"),
        ("hexdigest(-2**70)", "ValueError: Cannot convert negative int"),
        ("digest(2**29)", "ValueError: length is too large"),
        ("hexdigest(2**29)", "ValueError: length is too large"),
        (
            "digest(2**64)",
            "OverflowError: Python int too large for C unsigned long",
        ),
    ] {
        assert_eq!(
            run_err(&format!("import hashlib\nhashlib.shake_128().{call}")),
            expected,
            "{call}"
        );
    }
}

/// The RustCrypto state layout is the dump format of SHA-1 and SHA-2 objects
/// (`crypto-common` keeps it stable within a `0.x` series), so a change here
/// is a `DUMP_VERSION` bump.
#[test]
fn rustcrypto_serialized_state_layout_is_pinned() {
    fn state<D: Digest + SerializableState>() -> String {
        let mut hasher = D::new();
        Digest::update(&mut hasher, b"abc");
        hasher.serialize().iter().fold(String::new(), |mut hex, byte| {
            write!(hex, "{byte:02x}").unwrap();
            hex
        })
    }
    assert_snapshot!(state::<sha1::Sha1>(), @"0123456789abcdeffedcba9876543210f0e1d2c3000000000000000003616263000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000");
    assert_snapshot!(state::<sha2::Sha224>(), @"d89e05c107d57c3617dd703039590ef7310bc0ff11155868a78ff964a44ffabe000000000000000003616263000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000");
    assert_snapshot!(state::<sha2::Sha256>(), @"67e6096a85ae67bb72f36e3c3af54fa57f520e518c68059babd9831f19cde05b000000000000000003616263000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000");
    assert_snapshot!(state::<sha2::Sha384>(), @"d89e05c15d9dbbcb07d57c362a299a6217dd70305a01599139590ef7d8ec2f15310bc0ff6726336711155868874ab48ea78ff9640d2e0cdba44ffabe1d48b547000000000000000000000000000000000361626300000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000");
    assert_snapshot!(state::<sha2::Sha512>(), @"08c9bcf367e6096a3ba7ca8485ae67bb2bf894fe72f36e3cf1361d5f3af54fa5d182e6ad7f520e511f6c3e2b8c68059b6bbd41fbabd9831f79217e1319cde05b000000000000000000000000000000000361626300000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000");
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
