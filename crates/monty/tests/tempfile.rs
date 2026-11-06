//! Temporary naming and cleanup across interpreter snapshots.

use monty::{Dump, MontyRun, RunProgress, Session, SessionRef, dump};
use monty_types::{
    CompileOptions, ExcType, ExtFunctionResult, FileMode, MontyException, MontyFileHandle, MontyObject, OsFunctionCall,
    OsPolicy, PrintWriter, RandomSeed, RandomStart, ResourceTracker,
};

/// Start without a filesystem, letting each test service the typed requests.
fn start(code: &str, random_start: RandomStart) -> RunProgress {
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .with_os_policy(OsPolicy {
            random_start,
            ..OsPolicy::default()
        })
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap()
}

/// Restore the exact suspended operation and its frozen frame.
fn restore(progress: &RunProgress) -> RunProgress {
    let bytes = dump("test.py", None, &[], SessionRef::Running(progress)).unwrap();
    let Session::Running(progress) = Dump::load(&bytes).unwrap().state else {
        panic!("expected suspended execution");
    };
    *progress
}

/// Answer an exclusive reservation with the existing file-handle carrier.
fn reserve(progress: RunProgress) -> RunProgress {
    let RunProgress::OsCall(call) = progress else {
        panic!("expected creation");
    };
    let OsFunctionCall::CreateExclusive(args) = &call.function_call else {
        panic!("expected exclusive request");
    };
    assert_eq!(args.permissions, 0o600);
    let reply = MontyObject::file_handle(MontyFileHandle {
        path: args.path.to_string(),
        mode: args.mode,
        position: 0,
    });
    call.resume(reply, PrintWriter::Stdout).unwrap()
}

/// Supply the same deterministic host entropy to two copies of a suspension.
fn seed(progress: RunProgress) -> RunProgress {
    let RunProgress::OsCall(call) = progress else {
        panic!("expected entropy");
    };
    assert!(matches!(call.function_call, OsFunctionCall::Urandom(_)));
    call.resume(MontyObject::bytes(vec![42; 2496]), PrintWriter::Stdout)
        .unwrap()
}

#[test]
fn import_and_directory_helpers_do_not_request_entropy() {
    let result = start(
        "import tempfile\n(tempfile.gettempdir(), tempfile.gettempprefix())",
        RandomStart::CallHost,
    );
    assert_eq!(
        result.into_complete().unwrap(),
        MontyObject::tuple([
            MontyObject::string("/tmp".to_owned()),
            MontyObject::string("tmp".to_owned())
        ])
    );
}

#[test]
fn entropy_and_creation_suspensions_survive_snapshots() {
    let waiting = start(
        "import tempfile\nf = tempfile.NamedTemporaryFile(delete=False)\nf.name",
        RandomStart::CallHost,
    );
    let loaded = restore(&waiting);
    let first = seed(waiting);
    let second = seed(loaded);
    let RunProgress::OsCall(call) = &first else {
        panic!("expected creation");
    };
    let OsFunctionCall::CreateExclusive(args) = &call.function_call else {
        panic!("expected exclusive request");
    };
    assert_eq!(args.mode, FileMode::WriteUpdate(true));
    let expected = MontyObject::string(args.path.to_string());
    let third = restore(&first);
    for progress in [first, second, third] {
        assert_eq!(reserve(progress).into_complete().unwrap(), expected);
    }
}

#[test]
fn pending_cleanup_survives_snapshot_after_explicit_close() {
    let code = "import tempfile\nwith tempfile.NamedTemporaryFile(mode='w', delete_on_close=False) as f:\n    f.close()\nf.closed";
    let created = start(code, RandomStart::Seed(RandomSeed::Int(42.into())));
    let cleanup = reserve(created);
    let loaded = restore(&cleanup);
    for progress in [cleanup, loaded] {
        let RunProgress::OsCall(call) = progress else {
            panic!("expected cleanup");
        };
        assert!(matches!(call.function_call, OsFunctionCall::Unlink(_)));
        assert_eq!(
            call.resume(MontyObject::none(), PrintWriter::Stdout)
                .unwrap()
                .into_complete()
                .unwrap(),
            MontyObject::bool(true)
        );
    }
}

#[test]
fn ordinary_and_instance_random_streams_are_unchanged() {
    let baseline = "import random\na = random.random()\nb = random.Random().random()\n(a, b, random.random(), random.Random().random())";
    let with_temp = "import random, tempfile\na = random.random()\nb = random.Random().random()\ntempfile.NamedTemporaryFile(mode='w', delete=False).close()\n(a, b, random.random(), random.Random().random())";
    let policy = || RandomStart::Seed(RandomSeed::Int(42.into()));
    let expected = start(baseline, policy()).into_complete().unwrap();
    assert_eq!(reserve(start(with_temp, policy())).into_complete().unwrap(), expected);
}

#[test]
fn restored_names_retry_collisions_without_reseeding() {
    let progress = start(
        "import tempfile\nf = tempfile.NamedTemporaryFile(mode='w', delete=False)\nf.name",
        RandomStart::Seed(RandomSeed::Int(42.into())),
    );
    let loaded = restore(&progress);
    let RunProgress::OsCall(call) = progress else {
        panic!("expected creation");
    };
    let first = call.function_call.fs_primary_path().unwrap().to_owned();
    let collision = ExtFunctionResult::Error(MontyException::new(
        ExcType::FileExistsError,
        Some("occupied".to_owned()),
    ));
    let next = call.resume(collision, PrintWriter::Stdout).unwrap();
    let loaded_next = match loaded {
        RunProgress::OsCall(call) => call
            .resume(
                ExtFunctionResult::Error(MontyException::new(
                    ExcType::FileExistsError,
                    Some("occupied".to_owned()),
                )),
                PrintWriter::Stdout,
            )
            .unwrap(),
        _ => panic!("expected restored creation"),
    };
    let name = reserve(next).into_complete().unwrap();
    assert_ne!(name, MontyObject::string(first));
    assert_eq!(reserve(loaded_next).into_complete().unwrap(), name);
}

#[test]
fn file_cleanup_state_survives_a_snapshot_inside_the_body() {
    let code = "import tempfile\nwith tempfile.NamedTemporaryFile(mode='w', delete_on_close=False) as f:\n    f.write('payload')\nf.closed";
    let writing = reserve(start(code, RandomStart::Seed(RandomSeed::Int(42.into()))));
    let loaded = restore(&writing);
    for progress in [writing, loaded] {
        let RunProgress::OsCall(call) = progress else {
            panic!("expected write");
        };
        assert!(matches!(call.function_call, OsFunctionCall::AppendText(_)));
        let cleanup = call.resume(MontyObject::int(7), PrintWriter::Stdout).unwrap();
        let RunProgress::OsCall(call) = cleanup else {
            panic!("expected cleanup after write");
        };
        assert!(matches!(call.function_call, OsFunctionCall::Unlink(_)));
        assert_eq!(
            call.resume(MontyObject::none(), PrintWriter::Stdout)
                .unwrap()
                .into_complete()
                .unwrap(),
            MontyObject::bool(true)
        );
    }
}

#[test]
fn invalid_host_entropy_is_catchable_and_retryable_without_creating_a_file() {
    let code = "import tempfile\ntry:\n    tempfile.NamedTemporaryFile(mode='w', delete=False)\nexcept RuntimeError:\n    pass\ntempfile.NamedTemporaryFile(mode='w', delete=False).name";
    let progress = start(code, RandomStart::CallHost);
    let RunProgress::OsCall(call) = progress else {
        panic!("expected entropy");
    };
    let retry = call
        .resume(MontyObject::bytes(vec![0; 1]), PrintWriter::Stdout)
        .unwrap();
    assert!(reserve(seed(retry)).into_complete().is_some());
}
