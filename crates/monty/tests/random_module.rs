//! `random` at the host boundary: repr output, entropy callbacks and state
//! preserved across snapshots and REPL feeds.
//!
//! Seeded values are pinned against a live CPython in `test_cases/`; these
//! tests cover only what a fixture cannot drive — the suspension itself.

use insta::assert_snapshot;
use monty::{Dump, MontyRepl, MontyRun, RunProgress, Session, SessionRef, dump};
use monty_types::{
    CompileOptions, ExcType, ExtFunctionResult, MontyException, MontyNode, MontyObject, MontyType, OsFunctionCall,
    PrintWriter, ResourceTracker, UrandomArgs,
};

/// Bytes of entropy an unseeded generator asks for: one MT19937 state vector.
const SEED_BYTES: usize = 2496;

/// A fixed "entropy" reply. Its top byte is non-zero, so CPython seeds
/// identically from `int.from_bytes(pattern, 'little')`, which is how
/// [`PATTERN_FIRST_RANDOM`] was obtained.
fn pattern() -> MontyObject {
    #[expect(clippy::cast_possible_truncation, reason = "reduced mod 256 first")]
    MontyObject::bytes((0..SEED_BYTES).map(|i| (i % 256) as u8).collect::<Vec<u8>>())
}

/// CPython's first `random()` after seeding from [`pattern`].
const PATTERN_FIRST_RANDOM: f64 = 0.246_986_487_449_397_1;

/// Generator state and constructors stay in the sandbox, including inside returned containers.
#[test]
fn random_instances_and_types_cross_as_repr() {
    let result = start("import random\n[random.Random(42), random.Random, type(random.Random()), int]")
        .into_complete()
        .unwrap();
    let Some(values) = result.as_ref().items() else {
        panic!("expected a list");
    };
    let MontyNode::Repr(instance) = values[0].node() else {
        panic!("expected an instance repr");
    };
    assert!(instance.starts_with("<random.Random object at 0x"));
    assert!(instance.ends_with('>'));
    assert_eq!(values[1], MontyObject::repr("<class 'random.Random'>".to_owned()));
    assert_eq!(values[2], values[1]);
    assert_eq!(values[3], MontyObject::type_object(MontyType::Int));
    assert!(MontyType::from_type_name("random.Random").is_none());
}

/// Starts `code` under suspend/resume execution.
fn start(code: &str) -> RunProgress {
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap()
}

/// Asserts `progress` is paused on the entropy call and hands it back.
fn expect_entropy_call(progress: RunProgress) -> monty::OsCall {
    match progress {
        RunProgress::OsCall(call) => {
            assert!(
                matches!(call.function_call, OsFunctionCall::Urandom(UrandomArgs { size: 2496 })),
                "expected os.urandom(2496), got {:?}",
                call.function_call
            );
            call
        }
        other => panic!("expected an OsCall suspension, got {other:?}"),
    }
}

#[test]
fn an_unseeded_draw_asks_the_host_for_one_state_vector() {
    let call = expect_entropy_call(start("import random\nrandom.random()"));
    let result = call
        .resume(pattern(), PrintWriter::Stdout)
        .unwrap()
        .into_complete()
        .unwrap();
    assert_eq!(result, MontyObject::float(PATTERN_FIRST_RANDOM));
}

#[test]
fn the_reply_seeds_every_later_draw_without_suspending_again() {
    let code = "import random\nfirst = random.random()\n[first, random.randint(1, 100), random.random() < 1.0]";
    let call = expect_entropy_call(start(code));
    let result = call
        .resume(pattern(), PrintWriter::Stdout)
        .unwrap()
        .into_complete()
        .unwrap();
    assert_eq!(
        result,
        MontyObject::list([
            MontyObject::float(PATTERN_FIRST_RANDOM),
            MontyObject::int(77),
            MontyObject::bool(true),
        ])
    );
}

#[test]
fn seeded_code_never_suspends() {
    let progress = start("import random\nrandom.seed(42)\nrandom.random()");
    assert_eq!(
        progress.into_complete().unwrap(),
        MontyObject::float(0.639_426_798_457_883_7)
    );
}

#[test]
fn explicit_seed_with_no_argument_reseeds_from_the_host() {
    let call = expect_entropy_call(start("import random\nrandom.seed(1)\nrandom.seed()\nrandom.random()"));
    let result = call
        .resume(pattern(), PrintWriter::Stdout)
        .unwrap()
        .into_complete()
        .unwrap();
    assert_eq!(result, MontyObject::float(PATTERN_FIRST_RANDOM));
}

#[test]
fn an_unseeded_instance_seeds_itself_even_when_nothing_else_holds_it() {
    // The instance is a temporary: only the suspension's pin keeps it alive.
    let call = expect_entropy_call(start("import random\nrandom.Random().random()"));
    let result = call
        .resume(pattern(), PrintWriter::Stdout)
        .unwrap()
        .into_complete()
        .unwrap();
    assert_eq!(result, MontyObject::float(PATTERN_FIRST_RANDOM));
}

#[test]
fn a_wrong_sized_reply_is_a_runtime_error() {
    let call = expect_entropy_call(start("import random\nrandom.random()"));
    let err = call
        .resume(MontyObject::bytes(vec![1, 2, 3]), PrintWriter::Stdout)
        .unwrap_err();
    assert_snapshot!(err.to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 2, in <module>
        random.random()
        ~~~~~~~~~~~~~~~
    RuntimeError: 'os.urandom' returned 3 bytes, expected 2496
    "#);

    let call = expect_entropy_call(start("import random\nrandom.random()"));
    let err = call
        .resume(MontyObject::string("nope".to_owned()), PrintWriter::Stdout)
        .unwrap_err();
    assert_snapshot!(err.to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 2, in <module>
        random.random()
        ~~~~~~~~~~~~~~~
    RuntimeError: 'os.urandom' must return bytes, not str
    "#);
}

#[test]
fn a_host_error_leaves_the_generator_unseeded_and_catchable() {
    let code = "\
import random
try:
    random.random()
except OSError as exc:
    caught = str(exc)
[caught, random.random()]";
    let call = expect_entropy_call(start(code));
    let error = ExtFunctionResult::Error(MontyException::new(ExcType::OSError, Some("no entropy".to_owned())));
    // The draw retries on the next call, so a second suspension follows the caught error.
    let call = expect_entropy_call(call.resume(error, PrintWriter::Stdout).unwrap());
    let result = call
        .resume(pattern(), PrintWriter::Stdout)
        .unwrap()
        .into_complete()
        .unwrap();
    assert_eq!(
        result,
        MontyObject::list([
            MontyObject::string("no entropy".to_owned()),
            MontyObject::float(PATTERN_FIRST_RANDOM),
        ])
    );
}

#[test]
fn a_dump_taken_while_waiting_for_entropy_resumes_the_stashed_draw() {
    let code = "import random\nrandom.choice(['a', 'b', 'c']) + random.choice('xyz')";
    let progress = start(code);
    let bytes = dump("test.py", None, SessionRef::Running(&progress)).unwrap();
    let Session::Running(loaded) = Dump::load(&bytes).unwrap().state else {
        panic!("expected a running session");
    };

    let from_original = expect_entropy_call(progress)
        .resume(pattern(), PrintWriter::Stdout)
        .unwrap()
        .into_complete()
        .unwrap();
    let from_loaded = expect_entropy_call(*loaded)
        .resume(pattern(), PrintWriter::Stdout)
        .unwrap()
        .into_complete()
        .unwrap();
    assert_eq!(from_original, from_loaded);
    // CPython: two `choice` draws after seeding from the same bytes.
    assert_eq!(from_original, MontyObject::string("ax".to_owned()));
}

#[test]
fn setstate_truncates_words_between_2_63_and_2_64_like_64_bit_cpython() {
    // Not a fixture: CPython on Windows has a 32-bit `unsigned long` and raises here.
    let code =
        "import random\nr = random.Random(0)\nr.setstate((3, (2**63 + 7,) * 624 + (0,), None))\nr.getstate()[1][:2]";
    let progress = start(code);
    assert_eq!(
        progress.into_complete().unwrap(),
        MontyObject::tuple([MontyObject::int(7), MontyObject::int(7)])
    );
}

#[test]
fn os_urandom_accepts_only_bytes_of_the_requested_length() {
    let urandom = |reply: MontyObject| match start("import os\nos.urandom(3)") {
        RunProgress::OsCall(call) => {
            assert!(matches!(
                call.function_call,
                OsFunctionCall::Urandom(UrandomArgs { size: 3 })
            ));
            call.resume(reply, PrintWriter::Stdout)
                .map(|p| p.into_complete().unwrap())
        }
        other => panic!("expected an OsCall suspension, got {other:?}"),
    };
    assert_eq!(
        urandom(MontyObject::bytes(vec![1, 2, 3])).unwrap(),
        MontyObject::bytes(vec![1, 2, 3])
    );
    assert_snapshot!(urandom(MontyObject::bytes(vec![1, 2, 3, 4, 5])).unwrap_err().to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 2, in <module>
        os.urandom(3)
        ~~~~~~~~~~~~~
    RuntimeError: 'os.urandom' returned 5 bytes, expected 3
    "#);
    assert_snapshot!(urandom(MontyObject::string("abc".to_owned())).unwrap_err().to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 2, in <module>
        os.urandom(3)
        ~~~~~~~~~~~~~
    RuntimeError: 'os.urandom' must return bytes, not str
    "#);
}

#[test]
fn standard_execution_has_no_host_to_ask() {
    let err = MontyRun::new(
        "import random\nrandom.random()".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap()
    .run_no_limits(vec![])
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "NotImplementedError: OS function 'os.urandom' not implemented with standard execution"
    );
}

#[test]
fn the_module_generator_persists_across_repl_feeds() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run("import random\nrandom.seed(5)", vec![], PrintWriter::Stdout)
        .unwrap();
    // CPython: random.seed(5); random.random()
    assert_eq!(
        repl.feed_run("import random\nrandom.random()", vec![], PrintWriter::Stdout)
            .unwrap(),
        MontyObject::float(0.622_901_694_889_701_9)
    );
    // The seeded generator also travels through a dump of the idle session.
    let bytes = dump("test.py", None, SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else {
        panic!("expected an idle session");
    };
    let expected = repl
        .feed_run("import random\nrandom.random()", vec![], PrintWriter::Stdout)
        .unwrap();
    let actual = restored
        .feed_run("import random\nrandom.random()", vec![], PrintWriter::Stdout)
        .unwrap();
    assert_eq!(actual, expected);
}
