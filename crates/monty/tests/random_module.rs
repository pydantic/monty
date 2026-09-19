//! Random initialization policies, host entropy and state across snapshots and REPL feeds.
//! `test_cases/random__*.py` covers algorithms against CPython.

use insta::assert_snapshot;
use monty::{Dump, MontyRepl, MontyRun, RunProgress, Session, SessionRef, dump};
use monty_types::{
    AutoOsCalls, CompileOptions, ExcType, ExtFunctionResult, MontyException, MontyObject, MontyType, OsFunctionCall,
    PrintWriter, RandomSeed, RandomStart, ResourceTracker, UrandomArgs,
    unstable::{self, MontyNode},
};

/// One MT19937 state vector requested from the host under `CallHost`.
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

fn runner_with(code: &str, start: RandomStart) -> MontyRun {
    let auto_os_calls = AutoOsCalls {
        random_start: start,
        ..AutoOsCalls::default()
    };
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .with_auto_os_calls(auto_os_calls)
}

fn run_seeded(code: &str, seed: RandomSeed) -> MontyObject {
    runner_with(code, RandomStart::Seed(seed))
        .run_no_limits(vec![])
        .unwrap()
}

fn start_call_host(code: &str) -> RunProgress {
    runner_with(code, RandomStart::CallHost)
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap()
}

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

/// Generator state and constructors stay in the sandbox, including inside returned containers.
#[test]
fn random_instances_and_types_cross_as_repr() {
    let result = start("import random\n[random.Random(42), random.Random, type(random.Random()), int]")
        .into_complete()
        .unwrap();
    let Some(values) = result.as_ref().items() else {
        panic!("expected a list");
    };
    let MontyNode::Repr(instance) = unstable::node(values[0]) else {
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

/// OS entropy should produce distinct sequences across runs.
#[test]
fn an_unseeded_draw_never_suspends() {
    let draw = || {
        start("import random\nrandom.random()")
            .into_complete()
            .expect("entropy is read in the sandbox")
    };
    let (first, second) = (draw(), draw());
    for value in [&first, &second] {
        let Some(value) = value.as_ref().as_float() else {
            panic!("expected a float, got {value:?}");
        };
        assert!((0.0..1.0).contains(&value));
    }
    assert_ne!(first, second);
}

#[test]
fn seeded_code_never_suspends() {
    let progress = start("import random\nrandom.seed(42)\nrandom.random()");
    assert_eq!(
        progress.into_complete().unwrap(),
        MontyObject::float(0.639_426_798_457_883_7)
    );
}

/// Expected values are CPython's `random.seed(s); random.random(), random.randint(1, 100)`.
#[test]
fn a_session_seed_matches_random_seed() {
    let code = "import random\n[random.random(), random.randint(1, 100)]";
    for (seed, expected) in [
        (RandomSeed::Int(42.into()), (0.639_426_798_457_883_7, 4)),
        (RandomSeed::Int((-42).into()), (0.639_426_798_457_883_7, 4)),
        (
            RandomSeed::Int(num_bigint::BigInt::from(2u8).pow(70)),
            (0.232_788_271_830_183_8, 54),
        ),
        (RandomSeed::Float(1.5), (0.551_763_726_942_059, 33)),
        (RandomSeed::Str("abc".to_owned()), (0.772_024_631_415_754_5, 72)),
        (RandomSeed::Bytes(b"abc".to_vec()), (0.772_024_631_415_754_5, 72)),
    ] {
        assert_eq!(
            run_seeded(code, seed.clone()),
            MontyObject::list([MontyObject::float(expected.0), MontyObject::int(expected.1)]),
            "seed {seed:?}"
        );
    }
    assert_eq!(
        run_seeded(
            "import random\nrandom.seed(42)\nrandom.random()",
            RandomSeed::Str("x".to_owned())
        ),
        MontyObject::float(0.639_426_798_457_883_7)
    );
}

/// Unseeded instances and seed() derive reproducible states distinct from each other and the module.
#[test]
fn derived_states_are_deterministic_and_distinct() {
    let code = "import random\n\
                a = random.Random().random()\n\
                b = random.Random().random()\n\
                m = random.random()\n\
                random.seed()\n\
                r = random.random()\n\
                [a, b, m, r, len({a, b, m, r})]";
    let first = run_seeded(code, RandomSeed::Int(42.into()));
    let second = run_seeded(code, RandomSeed::Int(42.into()));
    assert_eq!(first, second);
    let Some(values) = first.as_ref().items() else {
        panic!("expected a list");
    };
    assert_eq!(values[2], MontyObject::float(0.639_426_798_457_883_7));
    assert_eq!(values[4], MontyObject::int(4));
}

#[test]
fn explicit_seed_with_no_argument_reseeds_from_entropy() {
    let result = start("import random\nrandom.seed(42)\nrandom.seed()\nrandom.random()")
        .into_complete()
        .unwrap();
    assert_ne!(result, MontyObject::float(0.639_426_798_457_883_7));
}

// ---------------------------------------------------------------------------
// `RandomStart::CallHost`: the host supplies the first state
// ---------------------------------------------------------------------------

#[test]
fn call_host_asks_the_host_for_one_state_vector() {
    let code = "import random\nfirst = random.random()\n[first, random.randint(1, 100), random.random() < 1.0]";
    let call = expect_entropy_call(start_call_host(code));
    let result = call
        .resume(pattern(), PrintWriter::Stdout)
        .unwrap()
        .into_complete()
        .unwrap();
    // the reply seeds every later draw without suspending again
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
fn call_host_seeds_an_explicit_seed_with_no_argument_from_the_host() {
    let call = expect_entropy_call(start_call_host(
        "import random\nrandom.seed(1)\nrandom.seed()\nrandom.random()",
    ));
    let result = call
        .resume(pattern(), PrintWriter::Stdout)
        .unwrap()
        .into_complete()
        .unwrap();
    assert_eq!(result, MontyObject::float(PATTERN_FIRST_RANDOM));
}

#[test]
fn call_host_seeds_an_unseeded_instance_even_when_nothing_else_holds_it() {
    // The instance is a temporary: only the suspension's pin keeps it alive.
    let call = expect_entropy_call(start_call_host("import random\nrandom.Random().random()"));
    let result = call
        .resume(pattern(), PrintWriter::Stdout)
        .unwrap()
        .into_complete()
        .unwrap();
    assert_eq!(result, MontyObject::float(PATTERN_FIRST_RANDOM));
}

#[test]
fn call_host_rejects_a_wrong_sized_reply() {
    let call = expect_entropy_call(start_call_host("import random\nrandom.random()"));
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

    let call = expect_entropy_call(start_call_host("import random\nrandom.random()"));
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

    // an output-only value cannot be imported at all; the contract's error still names it
    let call = expect_entropy_call(start_call_host("import random\nrandom.random()"));
    let err = call
        .resume(MontyObject::repr("<thing>"), PrintWriter::Stdout)
        .unwrap_err();
    assert_snapshot!(err.to_string(), @r#"
    Traceback (most recent call last):
      File "test.py", line 2, in <module>
        random.random()
        ~~~~~~~~~~~~~~~
    RuntimeError: 'os.urandom' must return bytes, not repr
    "#);
}

#[test]
fn call_host_error_leaves_the_generator_unseeded_and_catchable() {
    let code = "\
import random
try:
    random.random()
except OSError as exc:
    caught = str(exc)
[caught, random.random()]";
    let call = expect_entropy_call(start_call_host(code));
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
fn call_host_dump_taken_while_waiting_for_entropy_resumes_the_stashed_draw() {
    let code = "import random\nrandom.choice(['a', 'b', 'c']) + random.choice('xyz')";
    let progress = start_call_host(code);
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
fn call_host_has_no_host_under_standard_execution() {
    let err = runner_with("import random\nrandom.random()", RandomStart::CallHost)
        .run_no_limits(vec![])
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "NotImplementedError: OS function 'os.urandom' not implemented with standard execution"
    );
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

#[test]
fn a_session_seed_applies_across_repl_feeds_and_dumps() {
    let auto_os_calls = AutoOsCalls {
        random_start: RandomStart::Seed(RandomSeed::Int(42.into())),
        ..AutoOsCalls::default()
    };
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default())
        .with_auto_os_calls(auto_os_calls);
    repl.feed_run("import random", vec![], PrintWriter::Stdout).unwrap();
    let bytes = dump("test.py", None, SessionRef::Idle(&repl)).unwrap();
    let Session::Idle(mut restored) = Dump::load(&bytes).unwrap().state else {
        panic!("expected an idle session");
    };
    for repl in [&mut repl, &mut restored] {
        assert_eq!(
            repl.feed_run("random.random()", vec![], PrintWriter::Stdout).unwrap(),
            MontyObject::float(0.639_426_798_457_883_7)
        );
    }
}
