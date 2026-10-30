//! Host-backed `secrets` helpers and their entropy conversion behavior.

use std::time::Duration;

use monty::{Dump, MontyRepl, MontyRun, ReplProgress, RunProgress, Session, SessionRef, dump};
use monty_types::{
    CompileOptions, ExcType, MontyException, MontyObject, OsFunctionCall, PrintWriter, ResourceLimits, ResourceTracker,
    UrandomArgs,
};

/// Starts sandbox code that will request entropy from the host.
fn start(code: &str) -> RunProgress {
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap()
}

/// Supplies a repeated byte pattern to every entropy call and records request sizes.
fn run_with_fixed_entropy(code: &str) -> (MontyObject, Vec<usize>) {
    let mut progress = start(code);
    let mut requests = Vec::new();
    loop {
        match progress {
            RunProgress::OsCall(call) => {
                let size = match &call.function_call {
                    OsFunctionCall::Urandom(UrandomArgs { size }) => usize::try_from(*size).unwrap(),
                    other => panic!("expected os.urandom, got {other:?}"),
                };
                requests.push(size);
                let bytes: Vec<u8> = (0..size)
                    .map(|index| u8::try_from(index % 251).expect("reduced to a byte"))
                    .collect();
                progress = call.resume(MontyObject::bytes(bytes), PrintWriter::Stdout).unwrap();
            }
            complete => return (complete.into_complete().unwrap(), requests),
        }
    }
}

#[test]
fn public_exports_omit_the_unimplemented_system_random() {
    assert_eq!(
        start("import secrets\nsecrets.__all__").into_complete().unwrap(),
        MontyObject::list(
            [
                "choice",
                "randbelow",
                "randbits",
                "token_bytes",
                "token_hex",
                "token_urlsafe",
                "compare_digest"
            ]
            .map(|name| MontyObject::string(name.to_owned()))
        )
    );
}

#[test]
fn token_bytes_reads_the_mutable_default_entropy_even_when_imported_directly() {
    let RunProgress::OsCall(call) =
        start("from secrets import token_bytes\nimport secrets\nsecrets.DEFAULT_ENTROPY = 1\ntoken_bytes()")
    else {
        panic!("expected an entropy request");
    };
    assert!(matches!(
        &call.function_call,
        OsFunctionCall::Urandom(UrandomArgs { size: 1 })
    ));
    assert_eq!(
        call.resume(MontyObject::bytes(vec![0x42]), PrintWriter::Stdout)
            .unwrap()
            .into_complete()
            .unwrap(),
        MontyObject::bytes(vec![0x42])
    );
}

#[test]
fn mutable_default_entropy_survives_dump_and_reload_while_suspended() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "import secrets\nsecrets.DEFAULT_ENTROPY = 1",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();
    let progress = repl
        .feed_start(
            "from secrets import token_bytes\nfirst = token_bytes()\nassert secrets.DEFAULT_ENTROPY == 1\nsecond = token_bytes()\nsecond",
            vec![],
            PrintWriter::Stdout,
        )
        .unwrap();
    let ReplProgress::OsCall(call) = &progress else {
        panic!("expected an entropy request");
    };
    assert!(matches!(
        &call.function_call,
        OsFunctionCall::Urandom(UrandomArgs { size: 1 })
    ));

    let bytes = dump("test.py", None, &[], SessionRef::Suspended(&progress)).unwrap();
    drop(progress.into_repl());
    let Session::Suspended(progress) = Dump::load(&bytes).unwrap().state else {
        panic!("expected the suspended REPL session to restore");
    };
    let ReplProgress::OsCall(call) = *progress else {
        panic!("expected the restored entropy request");
    };
    assert!(matches!(
        &call.function_call,
        OsFunctionCall::Urandom(UrandomArgs { size: 1 })
    ));
    let ReplProgress::OsCall(call) = call
        .resume(MontyObject::bytes(vec![0x42]), PrintWriter::Stdout)
        .unwrap()
    else {
        panic!("expected the restored feed to request entropy again");
    };
    assert!(matches!(
        &call.function_call,
        OsFunctionCall::Urandom(UrandomArgs { size: 1 })
    ));
    let ReplProgress::Complete { value, .. } = call
        .resume(MontyObject::bytes(vec![0x24]), PrintWriter::Stdout)
        .unwrap()
    else {
        panic!("expected the restored feed to complete");
    };
    assert_eq!(value, MontyObject::bytes(vec![0x24]));
}

#[test]
fn mutable_default_entropy_persists_between_feed_run_calls() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "import secrets\nmarker = []\nsecrets.DEFAULT_ENTROPY = marker",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();
    assert_eq!(
        repl.feed_run("secrets.DEFAULT_ENTROPY is marker", vec![], PrintWriter::Stdout)
            .unwrap(),
        MontyObject::bool(true)
    );
}

#[test]
fn mutable_default_entropy_persists_across_repl_feeds_and_snapshots() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "import secrets\nsecrets.DEFAULT_ENTROPY = 1",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();
    let ReplProgress::OsCall(call) = repl
        .feed_start(
            "from secrets import token_bytes\ntoken_bytes()",
            vec![],
            PrintWriter::Stdout,
        )
        .unwrap()
    else {
        panic!("expected an entropy request");
    };
    assert!(matches!(
        &call.function_call,
        OsFunctionCall::Urandom(UrandomArgs { size: 1 })
    ));
    let ReplProgress::Complete { value, .. } = call
        .resume(MontyObject::bytes(vec![0x42]), PrintWriter::Stdout)
        .unwrap()
    else {
        panic!("expected the suspended REPL feed to complete");
    };
    assert_eq!(value, MontyObject::bytes(vec![0x42]));
}

#[test]
fn randbelow_accepts_its_cpython_keyword_name() {
    let RunProgress::OsCall(call) = start("import secrets\nsecrets.randbelow(exclusive_upper_bound=3)") else {
        panic!("expected an entropy request");
    };
    assert!(matches!(
        &call.function_call,
        OsFunctionCall::Urandom(UrandomArgs { size: 1 })
    ));
    assert_eq!(
        call.resume(MontyObject::bytes(vec![0]), PrintWriter::Stdout)
            .unwrap()
            .into_complete()
            .unwrap(),
        MontyObject::int(0)
    );
}

#[test]
fn host_reply_conversion_is_charged_to_the_execution_limit() {
    let code = "import secrets\nsecrets.token_hex(5_000_000)";
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let limits = ResourceLimits::default().max_feed_duration(Duration::from_millis(50));
    let RunProgress::OsCall(call) = run
        .start(vec![], ResourceTracker::new(limits), PrintWriter::Stdout)
        .unwrap()
    else {
        panic!("expected an entropy request before the host reply conversion");
    };
    let result = call.resume(MontyObject::bytes(vec![0; 5_000_000]), PrintWriter::Stdout);
    let error = result.expect_err("large conversion should exceed the execution-time budget");
    assert_eq!(error.exc_type(), ExcType::TimeoutError);
}

#[test]
fn token_helpers_convert_host_entropy_as_expected() {
    let (result, requests) = run_with_fixed_entropy(
        "import secrets\n[secrets.token_bytes(4), secrets.token_hex(4), secrets.token_urlsafe(4), secrets.randbits(13)]",
    );
    assert_eq!(requests, [4, 4, 4, 2]);
    assert_eq!(
        result,
        MontyObject::list([
            MontyObject::bytes(vec![0, 1, 2, 3]),
            MontyObject::string("00010203".to_owned()),
            MontyObject::string("AAECAw".to_owned()),
            MontyObject::int(0),
        ])
    );
}

#[test]
fn entropy_candidates_use_the_high_requested_bits() {
    let RunProgress::OsCall(bits) =
        start("import secrets\n[secrets.randbits(13), secrets.randbelow(3), secrets.choice('abc')]")
    else {
        panic!("expected randbits entropy request");
    };
    let RunProgress::OsCall(below) = bits
        .resume(MontyObject::bytes(vec![0xab, 0xcd]), PrintWriter::Stdout)
        .unwrap()
    else {
        panic!("expected randbelow entropy request");
    };
    let RunProgress::OsCall(choice) = below
        .resume(MontyObject::bytes(vec![0xbf]), PrintWriter::Stdout)
        .unwrap()
    else {
        panic!("expected choice entropy request");
    };
    assert_eq!(
        choice
            .resume(MontyObject::bytes(vec![0xbf]), PrintWriter::Stdout)
            .unwrap()
            .into_complete()
            .unwrap(),
        MontyObject::list([
            MontyObject::int(0x1579),
            MontyObject::int(2),
            MontyObject::string("c".to_owned()),
        ])
    );
}

#[test]
fn randbelow_retries_rejected_samples_and_supports_big_bounds() {
    let RunProgress::OsCall(first) = start("import secrets\nsecrets.randbelow(3)") else {
        panic!("expected the first entropy request");
    };
    assert!(matches!(
        &first.function_call,
        OsFunctionCall::Urandom(UrandomArgs { size: 1 })
    ));
    let RunProgress::OsCall(second) = first
        .resume(MontyObject::bytes(vec![0xff]), PrintWriter::Stdout)
        .unwrap()
    else {
        panic!("an out-of-range sample should retry");
    };
    assert_eq!(
        second
            .resume(MontyObject::bytes(vec![0]), PrintWriter::Stdout)
            .unwrap()
            .into_complete()
            .unwrap(),
        MontyObject::int(0)
    );

    let RunProgress::OsCall(first) = start("import secrets\nsecrets.randbelow(2**100)") else {
        panic!("expected a large-bound entropy request");
    };
    assert!(matches!(
        &first.function_call,
        OsFunctionCall::Urandom(UrandomArgs { size: 13 })
    ));
    let RunProgress::OsCall(second) = first
        .resume(MontyObject::bytes(vec![0xff; 13]), PrintWriter::Stdout)
        .unwrap()
    else {
        panic!("an out-of-range large sample should retry");
    };
    assert!(matches!(
        &second.function_call,
        OsFunctionCall::Urandom(UrandomArgs { size: 13 })
    ));
    assert_eq!(
        second
            .resume(MontyObject::bytes(vec![0; 13]), PrintWriter::Stdout)
            .unwrap()
            .into_complete()
            .unwrap(),
        MontyObject::int(0)
    );
}

#[test]
fn caught_entropy_errors_release_choice_and_allow_a_subsequent_draw() {
    let RunProgress::OsCall(first) = start(
        "import secrets\nitem = []\ntry:\n    secrets.choice([item])\nexcept RuntimeError:\n    pass\nsecrets.choice([item]) is item",
    ) else {
        panic!("expected the first entropy request");
    };
    let RunProgress::OsCall(second) = first
        .resume(
            MontyException::new(ExcType::RuntimeError, Some("entropy failed".to_owned())),
            PrintWriter::Stdout,
        )
        .unwrap()
    else {
        panic!("the caught host error should continue to the second draw");
    };
    assert_eq!(
        second
            .resume(MontyObject::bytes(vec![0]), PrintWriter::Stdout)
            .unwrap()
            .into_complete()
            .unwrap(),
        MontyObject::bool(true)
    );
}

#[test]
fn choice_uses_rejection_sampling_and_returns_sequence_item() {
    let (result, requests) = run_with_fixed_entropy("import secrets\nsecrets.choice('abc')");
    assert_eq!(requests, [1]);
    assert_eq!(result, MontyObject::string("a".to_owned()));
}
