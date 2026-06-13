//! End-to-end pool tests against the real `monty` binary — including the
//! headline scenarios: a worker dying mid-execution (kill, crash, timeout)
//! must surface as a clean error and never poison the pool.

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
    sync::Once,
    thread,
    time::Duration,
};
#[cfg(unix)]
use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

use monty::{MontyObject, PrintStream, ResourceLimits};
use monty_pool::{MountSpec, MountSpecMode, Pool, PoolConfig, PoolError, ReplConfig, ResumeValue, TurnEvent};

/// Locates (building once if needed) the `monty` CLI binary for tests.
fn monty_binary() -> PathBuf {
    static BUILD: Once = Once::new();
    if let Ok(path) = env::var("MONTY_TEST_BIN") {
        return PathBuf::from(path);
    }
    // <workspace>/target/debug/monty, derived from this crate's manifest dir
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("target/debug/monty");
    BUILD.call_once(|| {
        if !path.exists() {
            let status = Command::new(env!("CARGO"))
                .args(["build", "-p", "monty-cli"])
                .status()
                .expect("failed to run cargo build -p monty-cli");
            assert!(status.success(), "building the monty binary failed");
        }
    });
    assert!(path.exists(), "monty binary missing at {}", path.display());
    path
}

fn config() -> PoolConfig {
    PoolConfig::new(monty_binary())
}

fn no_print(_: PrintStream, _: &str) {}

#[track_caller]
fn expect_complete(event: TurnEvent) -> MontyObject {
    match event {
        TurnEvent::Complete(value) => value,
        other => panic!("expected Complete, got {other:?}"),
    }
}

/// Kills a process by pid with SIGKILL — simulates a hard crash.
fn kill_pid(pid: u32) {
    #[cfg(unix)]
    {
        let status = Command::new("kill").args(["-9", &pid.to_string()]).status().unwrap();
        assert!(status.success());
    }
    #[cfg(windows)]
    {
        let status = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .status()
            .unwrap();
        assert!(status.success());
    }
}

// =============================================================================
// Happy path
// =============================================================================

#[test]
fn feed_and_finish_reuses_the_worker() {
    let pool = Pool::new(config()).unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    let first_pid = session.pid().unwrap();

    let event = session
        .feed("x = 40\nx + 2", vec![], vec![], false, &mut no_print)
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::Int(42));
    // session state persists across feeds on the same checkout
    let event = session.feed("x * 2", vec![], vec![], false, &mut no_print).unwrap();
    assert_eq!(expect_complete(event), MontyObject::Int(80));
    session.finish().unwrap();
    assert_eq!(pool.idle_workers(), 1);

    // the same process serves the next checkout, with a FRESH session:
    // `x` from the previous session must not leak, so reading it suspends
    // at NameLookup and resolving it as undefined raises NameError
    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    assert_eq!(session.pid().unwrap(), first_pid);
    let event = session.feed("x", vec![], vec![], false, &mut no_print).unwrap();
    assert!(matches!(event, TurnEvent::NameLookup { name } if name == "x"));
    let err = session.resume_name_lookup(None, &mut no_print).unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.message(), Some("name 'x' is not defined"));
    session.finish().unwrap();
}

#[test]
fn cyclic_return_value_decodes_and_keeps_the_worker_alive() {
    let pool = Pool::new(config()).unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    // a cyclic dict completes with a `Cycle` placeholder in the payload; the
    // parent must decode it rather than discarding the worker as misbehaving
    let event = session
        .feed("d = {}\nd['self'] = d\nd", vec![], vec![], false, &mut no_print)
        .unwrap();
    let MontyObject::Dict(pairs) = expect_complete(event) else {
        panic!("expected Dict");
    };
    let pairs: Vec<_> = pairs.into_iter().collect();
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].0, MontyObject::String("self".to_owned()));
    assert!(matches!(&pairs[0].1, MontyObject::Cycle(_, placeholder) if placeholder == "{...}"));
    // the session must still be usable on the same worker
    let event = session.feed("1 + 1", vec![], vec![], false, &mut no_print).unwrap();
    assert_eq!(expect_complete(event), MontyObject::Int(2));
    session.finish().unwrap();
    assert_eq!(pool.idle_workers(), 1);
}

#[test]
fn name_lookup_value_too_deep_for_the_wire_is_rejected_cleanly() {
    let pool = Pool::new(config()).unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    let event = session.feed("missing", vec![], vec![], false, &mut no_print).unwrap();
    assert!(matches!(event, TurnEvent::NameLookup { ref name } if name == "missing"));
    // a value nested past the wire depth bound would produce a frame the
    // worker cannot decode; it must fail as a session-preserving error
    let deep = (0..=monty_pool::MAX_VALUE_DEPTH).fold(MontyObject::Int(1), |inner, _| MontyObject::List(vec![inner]));
    let err = session.resume_name_lookup(Some(deep), &mut no_print).unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.message(), Some("Max input depth exceeded"));
    // the suspension is still pending, so a sendable answer completes the feed
    let event = session
        .resume_name_lookup(Some(MontyObject::Int(7)), &mut no_print)
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::Int(7));
    session.finish().unwrap();
}

/// A mount whose host path is not valid UTF-8 cannot cross the wire (the
/// proto `host_path` is a UTF-8 `string`). It must fail as a
/// session-preserving error rather than silently transcoding to a different —
/// possibly existing — path. Unix-only: non-UTF-8 paths cannot be constructed
/// portably.
#[cfg(unix)]
#[test]
fn non_utf8_mount_path_is_rejected_cleanly() {
    let pool = Pool::new(config()).unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    let bad_path = PathBuf::from(OsStr::from_bytes(b"/tmp/\xff"));
    let err = session
        .feed(
            "1 + 1",
            vec![],
            vec![MountSpec {
                virtual_path: "/mnt/data".to_owned(),
                host_path: bad_path,
                mode: MountSpecMode::ReadOnly,
                write_bytes_limit: None,
            }],
            false,
            &mut no_print,
        )
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert!(
        exc.message().is_some_and(|m| m.contains("not valid UTF-8")),
        "unexpected message: {:?}",
        exc.message()
    );
    // nothing was sent, so the worker is still synced and the session usable
    let event = session.feed("1 + 1", vec![], vec![], false, &mut no_print).unwrap();
    assert_eq!(expect_complete(event), MontyObject::Int(2));
    session.finish().unwrap();
}

/// An over-limit frame must fail as a clean, session-preserving error rather
/// than crashing the worker: `write_frame` rejects it before writing any
/// bytes, so the stream stays synced. Covers both directions — a request the
/// parent cannot send (a huge input) and a result the worker cannot send
/// back.
///
/// Allocates ~257 MiB (just over the 256 MiB frame limit) in the test process
/// and again in the worker, so it is memory-heavy; disable it if it proves
/// flaky in CI.
#[test]
fn oversize_frames_are_rejected_without_killing_the_worker() {
    // just over monty_proto's 256 MiB MAX_FRAME_LEN
    const OVERSIZE: usize = 257 * 1024 * 1024;

    let pool = Pool::new(config()).unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).unwrap();

    // (1) parent -> child: an input larger than the frame limit cannot be
    // sent. The worker never receives the request, so the session survives.
    let huge = MontyObject::String("x".repeat(OVERSIZE));
    let err = session
        .feed("data", vec![("data".to_owned(), huge)], vec![], false, &mut no_print)
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime for oversize input, got {err:?}");
    };
    assert!(
        exc.message()
            .is_some_and(|m| m.contains("request frame") && m.contains("exceeds the maximum")),
        "unexpected message: {:?}",
        exc.message()
    );
    let event = session.feed("1 + 1", vec![], vec![], false, &mut no_print).unwrap();
    assert_eq!(expect_complete(event), MontyObject::Int(2));

    // (2) child -> parent: a result larger than the frame limit cannot be sent
    // back. The worker answers with a clean error and keeps the session.
    let err = session
        .feed(&format!("'x' * {OVERSIZE}"), vec![], vec![], false, &mut no_print)
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime for oversize result, got {err:?}");
    };
    assert!(
        exc.message()
            .is_some_and(|m| m.contains("result frame") && m.contains("exceeds the maximum")),
        "unexpected message: {:?}",
        exc.message()
    );
    let event = session.feed("1 + 1", vec![], vec![], false, &mut no_print).unwrap();
    assert_eq!(expect_complete(event), MontyObject::Int(2));

    // (3) child -> parent suspension: external-call arguments larger than the
    // frame limit cannot be announced to the parent. The child aborts that
    // feed before entering the suspension, so the session remains usable.
    let err = session
        .feed(
            &format!(
                "result = 'not caught'\ntry:\n    fetch('x' * {OVERSIZE})\nexcept RuntimeError:\n    result = 'caught'\nresult"
            ),
            vec![],
            vec![],
            false,
            &mut no_print,
        )
        .unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime for oversize external-call args, got {err:?}");
    };
    assert!(
        exc.message()
            .is_some_and(|m| m.contains("argument frame") && m.contains("exceeds the maximum")),
        "unexpected message: {:?}",
        exc.message()
    );
    let event = session.feed("1 + 1", vec![], vec![], false, &mut no_print).unwrap();
    assert_eq!(expect_complete(event), MontyObject::Int(2));

    session.finish().unwrap();
}

#[test]
fn inputs_and_prints() {
    let pool = Pool::new(config()).unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    let mut output = String::new();
    let event = session
        .feed(
            "print('hello', name)\nlen(name)",
            vec![("name".to_owned(), MontyObject::String("monty".to_owned()))],
            vec![],
            false,
            &mut |_, text| output.push_str(text),
        )
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::Int(5));
    assert_eq!(output, "hello monty\n");
    session.finish().unwrap();
}

#[test]
fn external_function_round_trip() {
    let pool = Pool::new(config()).unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    let event = session
        .feed("fetch('https://x') + '!'", vec![], vec![], false, &mut no_print)
        .unwrap();
    let TurnEvent::FunctionCall {
        function_name, args, ..
    } = event
    else {
        panic!("expected FunctionCall, got {event:?}");
    };
    assert_eq!(function_name, "fetch");
    assert_eq!(args, vec![MontyObject::String("https://x".to_owned())]);
    let event = session
        .resume(
            ResumeValue::Return(MontyObject::String("body".to_owned())),
            &mut no_print,
        )
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::String("body!".to_owned()));
    session.finish().unwrap();
}

#[test]
fn runtime_error_keeps_session_and_worker() {
    let pool = Pool::new(config()).unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    assert_eq!(
        expect_complete(session.feed("kept = 1", vec![], vec![], false, &mut no_print).unwrap()),
        MontyObject::None
    );
    let err = session.feed("1 / 0", vec![], vec![], false, &mut no_print).unwrap_err();
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.message(), Some("division by zero"));
    // session and worker survive
    assert_eq!(
        expect_complete(session.feed("kept + 41", vec![], vec![], false, &mut no_print).unwrap()),
        MontyObject::Int(42)
    );
    session.finish().unwrap();
    assert_eq!(pool.idle_workers(), 1);
}

// =============================================================================
// Crash isolation — the headline feature
// =============================================================================

#[test]
fn sigkill_mid_request_is_a_clean_crash_and_the_pool_recovers() {
    let pool = Pool::new(config()).unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    let pid = session.pid().unwrap();

    // kill the worker while it spins forever
    let killer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        kill_pid(pid);
    });
    let err = session
        .feed("while True:\n    pass", vec![], vec![], false, &mut no_print)
        .unwrap_err();
    killer.join().unwrap();
    assert!(matches!(err, PoolError::Crashed { .. }), "got {err:?}");

    // the pool spawns a replacement transparently
    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    assert_ne!(session.pid().unwrap(), pid);
    assert_eq!(
        expect_complete(session.feed("1 + 1", vec![], vec![], false, &mut no_print).unwrap()),
        MontyObject::Int(2)
    );
    session.finish().unwrap();
}

#[test]
fn worker_killed_while_idle_is_replaced_transparently() {
    let pool = Pool::new(config()).unwrap();
    let pids = pool.idle_worker_pids();
    assert_eq!(pids.len(), 1);
    kill_pid(pids[0]);
    // give the OS a moment to deliver the kill
    thread::sleep(Duration::from_millis(100));

    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    assert_eq!(
        expect_complete(session.feed("2 + 2", vec![], vec![], false, &mut no_print).unwrap()),
        MontyObject::Int(4)
    );
    session.finish().unwrap();
}

/// A deep postfix spine (`a.x.x.x…`) currently overflows the child's stack
/// (recursive AST handling) — a REAL memory crash, exactly what subprocess
/// isolation exists for. If monty core gains AST-depth protection this
/// becomes a `Runtime`/`Crashed` either way; the invariant under test is
/// that the parent survives and the pool keeps serving.
#[test]
fn hard_child_crash_does_not_harm_the_pool() {
    let pool = Pool::new(config()).unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    let mut code = String::with_capacity(300_002);
    code.push('a');
    for _ in 0..150_000 {
        code.push_str(".x");
    }
    let err = session.feed(&code, vec![], vec![], false, &mut no_print).unwrap_err();
    assert!(
        matches!(err, PoolError::Crashed { .. } | PoolError::Runtime(_)),
        "got {err:?}"
    );

    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    assert_eq!(
        expect_complete(session.feed("1 + 1", vec![], vec![], false, &mut no_print).unwrap()),
        MontyObject::Int(2)
    );
    session.finish().unwrap();
}

#[test]
fn watchdog_kills_hung_worker_after_request_timeout() {
    let mut config = config();
    config.request_timeout = Some(Duration::from_millis(300));
    let pool = Pool::new(config).unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    let err = session
        .feed("while True:\n    pass", vec![], vec![], false, &mut no_print)
        .unwrap_err();
    assert!(matches!(err, PoolError::Timeout { .. }), "got {err:?}");

    // pool still healthy
    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    assert_eq!(
        expect_complete(session.feed("3 + 3", vec![], vec![], false, &mut no_print).unwrap()),
        MontyObject::Int(6)
    );
    session.finish().unwrap();
}

#[test]
fn child_resource_limits_do_not_kill_the_worker() {
    let pool = Pool::new(config()).unwrap();
    let mut session = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::new().max_duration(Duration::from_millis(100))),
            ..ReplConfig::default()
        })
        .unwrap();
    let err = session
        .feed("while True:\n    pass", vec![], vec![], false, &mut no_print)
        .unwrap_err();
    // the SANDBOX limit fired (TimeoutError exception), not the watchdog —
    // the worker process is alive and finishable
    let PoolError::Runtime(exc) = err else {
        panic!("expected Runtime, got {err:?}");
    };
    assert_eq!(exc.exc_type().to_string(), "TimeoutError");
    session.finish().unwrap();
    assert_eq!(pool.idle_workers(), 1);
}

#[cfg(unix)]
#[test]
fn duration_backstop_kills_a_worker_blocked_in_a_syscall() {
    // Reading a FIFO blocks the worker inside the OS, where the sandbox's
    // periodic time check can never run — the parent-side `max_duration`
    // backstop (remaining budget + grace) is the only thing that can end the
    // turn. Note no `request_timeout` is configured here.
    let dir = tempfile::tempdir().unwrap();
    let status = Command::new("mkfifo").arg(dir.path().join("pipe")).status().unwrap();
    assert!(status.success(), "mkfifo failed");

    let mut config = config();
    config.duration_limit_grace = Some(Duration::from_millis(300));
    let pool = Pool::new(config).unwrap();
    let mut session = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::new().max_duration(Duration::from_millis(100))),
            ..ReplConfig::default()
        })
        .unwrap();
    let err = session
        .feed(
            "from pathlib import Path\nPath('/mnt/pipe').read_text()",
            vec![],
            vec![MountSpec {
                virtual_path: "/mnt".to_owned(),
                host_path: dir.path().to_path_buf(),
                mode: MountSpecMode::ReadOnly,
                write_bytes_limit: None,
            }],
            false,
            &mut no_print,
        )
        .unwrap_err();
    let PoolError::Timeout { timeout } = err else {
        panic!("expected Timeout, got {err:?}");
    };
    // the armed deadline was the remaining budget (≤100ms) plus the grace
    assert!(timeout <= Duration::from_millis(400), "deadline was {timeout:?}");
}

#[test]
fn suspension_time_does_not_consume_the_duration_budget() {
    // `max_duration` measures cumulative sandbox execution time; the worker
    // reports it on every turn and its clock is paused while suspended. The
    // host staying away for twice the entire budget must therefore not time
    // the session out.
    let pool = Pool::new(config()).unwrap();
    let mut session = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::new().max_duration(Duration::from_millis(300))),
            ..ReplConfig::default()
        })
        .unwrap();
    let event = session
        .feed("fetch('https://x') + '!'", vec![], vec![], false, &mut no_print)
        .unwrap();
    assert!(matches!(event, TurnEvent::FunctionCall { .. }));

    thread::sleep(Duration::from_millis(600));

    let event = session
        .resume(
            ResumeValue::Return(MontyObject::String("body".to_owned())),
            &mut no_print,
        )
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::String("body!".to_owned()));
    session.finish().unwrap();
}

#[cfg(unix)]
#[test]
fn loaded_session_keeps_its_duration_budget_for_the_backstop() {
    // The `max_duration` budget and consumed execution time travel inside the
    // dump, and the worker stamps them onto its replies — so a session
    // restored via `checkout_load` regains the parent-side backstop without
    // the parent ever having seen the original `ReplConfig`.
    let dir = tempfile::tempdir().unwrap();
    let status = Command::new("mkfifo").arg(dir.path().join("pipe")).status().unwrap();
    assert!(status.success(), "mkfifo failed");

    let mut config = config();
    config.duration_limit_grace = Some(Duration::from_millis(300));
    let pool = Pool::new(config).unwrap();
    let mut session = pool
        .checkout(&ReplConfig {
            limits: Some(ResourceLimits::new().max_duration(Duration::from_millis(100))),
            ..ReplConfig::default()
        })
        .unwrap();
    let state = session.dump().unwrap();
    drop(session);

    let (mut restored, event) = pool.checkout_load(state).unwrap();
    assert!(event.is_none(), "idle dump should restore without a suspension");
    let err = restored
        .feed(
            "from pathlib import Path\nPath('/mnt/pipe').read_text()",
            vec![],
            vec![MountSpec {
                virtual_path: "/mnt".to_owned(),
                host_path: dir.path().to_path_buf(),
                mode: MountSpecMode::ReadOnly,
                write_bytes_limit: None,
            }],
            false,
            &mut no_print,
        )
        .unwrap_err();
    let PoolError::Timeout { timeout } = err else {
        panic!("expected Timeout, got {err:?}");
    };
    assert!(timeout <= Duration::from_millis(400), "deadline was {timeout:?}");
}

// =============================================================================
// Lifecycle
// =============================================================================

#[test]
fn elastic_growth_up_to_max_then_exhausted() {
    let mut config = config();
    config.min_processes = 1;
    config.max_processes = 2;
    config.checkout_timeout = Some(Duration::from_millis(200));
    let pool = Pool::new(config).unwrap();

    let one = pool.checkout(&ReplConfig::default()).unwrap();
    let two = pool.checkout(&ReplConfig::default()).unwrap(); // grows beyond min
    let Err(err) = pool.checkout(&ReplConfig::default()) else {
        panic!("expected Exhausted");
    };
    assert!(matches!(err, PoolError::Exhausted), "got {err:?}");

    one.finish().unwrap();
    let three = pool.checkout(&ReplConfig::default()).unwrap(); // reuses one's worker
    three.finish().unwrap();
    two.finish().unwrap();
    assert_eq!(pool.idle_workers(), 2);
}

#[test]
fn dropping_a_checkout_kills_the_worker_but_frees_capacity() {
    let mut config = config();
    config.max_processes = 1;
    let pool = Pool::new(config).unwrap();

    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    let pid = session.pid().unwrap();
    let _ = session.feed("x = 1", vec![], vec![], false, &mut no_print).unwrap();
    drop(session); // abandoned mid-session: worker killed, capacity released

    // with max_processes=1 this checkout only succeeds if capacity was freed
    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    assert_ne!(session.pid().unwrap(), pid);
    assert_eq!(
        expect_complete(session.feed("5 + 5", vec![], vec![], false, &mut no_print).unwrap()),
        MontyObject::Int(10)
    );
    session.finish().unwrap();
}

#[test]
fn workers_are_recycled_after_max_checkouts() {
    let mut config = config();
    config.max_checkouts_per_worker = Some(1);
    let pool = Pool::new(config).unwrap();

    let session = pool.checkout(&ReplConfig::default()).unwrap();
    let first_pid = session.pid().unwrap();
    session.finish().unwrap();
    assert_eq!(pool.idle_workers(), 0, "worker must be retired, not pooled");

    let session = pool.checkout(&ReplConfig::default()).unwrap();
    assert_ne!(session.pid().unwrap(), first_pid);
    session.finish().unwrap();
}

#[test]
fn concurrent_checkouts_run_in_parallel() {
    let mut config = config();
    config.min_processes = 2;
    config.max_processes = 2;
    let pool = Pool::new(config).unwrap();

    thread::scope(|scope| {
        for _ in 0..2 {
            scope.spawn(|| {
                let mut session = pool.checkout(&ReplConfig::default()).unwrap();
                let event = session
                    .feed("sum(range(1000))", vec![], vec![], false, &mut no_print)
                    .unwrap();
                assert_eq!(expect_complete(event), MontyObject::Int(499_500));
                session.finish().unwrap();
            });
        }
    });
    assert_eq!(pool.idle_workers(), 2);
}

#[test]
fn typing_error_via_pool() {
    let pool = Pool::new(config()).unwrap();
    let mut session = pool
        .checkout(&ReplConfig {
            type_check: true,
            ..ReplConfig::default()
        })
        .unwrap();
    let err = session
        .feed("x: int = 'nope'", vec![], vec![], false, &mut no_print)
        .unwrap_err();
    let PoolError::Typing(diagnostics) = err else {
        panic!("expected Typing, got {err:?}");
    };
    assert!(diagnostics.contains("invalid-assignment"), "{diagnostics}");
    // the session survives a typing rejection
    assert_eq!(
        expect_complete(session.feed("1 + 1", vec![], vec![], false, &mut no_print).unwrap()),
        MontyObject::Int(2)
    );
    session.finish().unwrap();
}

// =============================================================================
// Dump / load across workers
// =============================================================================

#[test]
fn dump_survives_worker_death_and_loads_elsewhere() {
    let pool = Pool::new(config()).unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    assert_eq!(
        expect_complete(session.feed("base = 40", vec![], vec![], false, &mut no_print).unwrap()),
        MontyObject::None
    );
    let event = session
        .feed("base + ext()", vec![], vec![], false, &mut no_print)
        .unwrap();
    let TurnEvent::FunctionCall {
        call_id: _,
        ref function_name,
        ..
    } = event
    else {
        panic!("expected FunctionCall, got {event:?}");
    };
    assert_eq!(function_name, "ext");

    let state = session.dump().unwrap();
    drop(session); // kill the original worker outright

    let (mut restored, event) = pool.checkout_load(state).unwrap();
    let Some(TurnEvent::FunctionCall { ref function_name, .. }) = event else {
        panic!("expected re-announced FunctionCall, got {event:?}");
    };
    assert_eq!(function_name, "ext");
    let event = restored
        .resume(ResumeValue::Return(MontyObject::Int(2)), &mut no_print)
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::Int(42));
    restored.finish().unwrap();
}

// =============================================================================
// Environment isolation
// =============================================================================

/// Workers must be spawned with an empty environment: host secrets must never
/// be in a worker's memory, where a sandbox escape or memory disclosure could
/// reach them. Linux-only because it observes the child via /proc.
#[cfg(target_os = "linux")]
#[test]
fn worker_environment_is_empty() {
    // The test process itself always carries variables (PATH, CARGO, ...),
    // so an empty child environ proves nothing was inherited.
    assert!(env::var("PATH").is_ok(), "test process should have PATH set");

    let pool = Pool::new(config()).unwrap();
    let mut session = pool.checkout(&ReplConfig::default()).unwrap();
    #[expect(clippy::absolute_paths)]
    let environ = std::fs::read(format!("/proc/{}/environ", session.pid().unwrap())).unwrap();
    assert!(
        environ.is_empty(),
        "worker environment should be empty, got: {}",
        String::from_utf8_lossy(&environ).replace('\0', " ")
    );

    // The worker is fully functional without an environment.
    let event = session.feed("1 + 1", vec![], vec![], false, &mut no_print).unwrap();
    assert_eq!(expect_complete(event), MontyObject::Int(2));
    session.finish().unwrap();
}
