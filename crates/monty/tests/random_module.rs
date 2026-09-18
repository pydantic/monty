//! `random` at the host boundary: repr output, how an unseeded generator
//! takes its first state (`RandomStart`), and state preserved across
//! snapshots and REPL feeds.
//!
//! Seeded values are pinned against a live CPython in `test_cases/`; these
//! tests cover only what a fixture cannot drive — the host-chosen start.

use insta::assert_snapshot;
use monty::{Dump, MontyRepl, MontyRun, RunProgress, Session, SessionRef, dump};
use monty_types::{
    AutoOsCalls, CompileOptions, MontyObject, MontyType, OsFunctionCall, PrintWriter, RandomSeed, RandomStart,
    ResourceTracker, UrandomArgs,
    unstable::{self, MontyNode},
};

/// A runner for `code` that starts `random` from `seed`.
fn seeded_runner(code: &str, seed: RandomSeed) -> MontyRun {
    let auto_os_calls = AutoOsCalls {
        random_start: RandomStart::Seed(seed),
        ..AutoOsCalls::default()
    };
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .with_auto_os_calls(auto_os_calls)
}

/// Runs `code` with `random` started from `seed`.
fn run_seeded(code: &str, seed: RandomSeed) -> MontyObject {
    seeded_runner(code, seed).run_no_limits(vec![]).unwrap()
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

/// An unseeded generator seeds itself from OS entropy on its first draw, so
/// nothing suspends and two runs disagree.
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

/// A seeded generator produces CPython's sequence and never suspends.
#[test]
fn seeded_code_never_suspends() {
    let progress = start("import random\nrandom.seed(42)\nrandom.random()");
    assert_eq!(
        progress.into_complete().unwrap(),
        MontyObject::float(0.639_426_798_457_883_7)
    );
}

/// `RandomStart::Seed(s)` starts the module generator exactly as
/// `random.seed(s)` would, for every seed type CPython accepts. The values
/// are CPython's: `random.seed(s); random.random(), random.randint(1, 100)`.
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
    // `random.seed(42)` in the sandbox lands on the same state
    assert_eq!(
        run_seeded(
            "import random\nrandom.seed(42)\nrandom.random()",
            RandomSeed::Str("x".to_owned())
        ),
        MontyObject::float(0.639_426_798_457_883_7)
    );
}

/// Under a session seed, unseeded instances and `seed()` take deterministic
/// states derived from it: the same from run to run, but distinct from the
/// module generator's and from each other.
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

/// `random.seed()` with no argument reseeds from entropy, so the draw that
/// follows no longer matches the explicit seed.
#[test]
fn explicit_seed_with_no_argument_reseeds_from_entropy() {
    let result = start("import random\nrandom.seed(42)\nrandom.seed()\nrandom.random()")
        .into_complete()
        .unwrap();
    assert_ne!(result, MontyObject::float(0.639_426_798_457_883_7));
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

/// The session seed applies to the first draw whichever feed makes it, and
/// travels through a dump like the generator itself.
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
