use std::sync::OnceLock;

use monty_proto::{WireArena, WireFunctionCall};
use monty_types::ExecutionClock;
#[cfg(target_os = "linux")]
use nix::{
    errno::Errno,
    sys::wait::{WaitPidFlag, waitpid},
    unistd::Pid,
};

use super::*;

fn supervised(binary: PathBuf) -> PoolConfig {
    let mut config = PoolConfig::subprocess(binary);
    config.execution_clock = ExecutionClock::Cpu;
    config.min_processes = 0;
    config.max_processes = 1;
    config.request_timeout = Some(Duration::from_secs(2));
    // Zero grace exposes any wall deadline incorrectly derived from a CPU budget.
    config.feed_duration_limit_grace = Some(Duration::ZERO);
    config.turn_duration_limit_grace = Some(Duration::ZERO);
    config
}

fn limits(millis: u64) -> ReplConfig {
    ReplConfig {
        limits: Some(
            ResourceLimits::default()
                .max_feed_duration(Duration::from_millis(millis))
                .max_turn_duration(Duration::from_millis(millis)),
        ),
        ..ReplConfig::default()
    }
}

#[cfg(target_os = "linux")]
fn reaped(pid: u32) {
    assert_eq!(
        waitpid(Pid::from_raw(i32::try_from(pid).unwrap()), Some(WaitPidFlag::WNOHANG)),
        Err(Errno::ECHILD)
    );
}

#[cfg(target_os = "macos")]
fn reaped(pid: u32) {
    assert!(
        !Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .unwrap()
            .status
            .success()
    );
}

/// Emits chosen protocol frames without interpreting their contents.
fn stand_in(action: &str, reported: u64) -> (tempfile::TempDir, PoolConfig) {
    // Resolve Python before the worker clears PATH, so CI's configured interpreter is used.
    static PYTHON: OnceLock<PathBuf> = OnceLock::new();
    let python = PYTHON.get_or_init(|| {
        let output = Command::new("python3")
            .args(["-I", "-S", "-c", "import sys; print(sys.executable)"])
            .output()
            .expect("find the test Python interpreter");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
    });
    let dir = tempfile::Builder::new().prefix("monty cpu '").tempdir().unwrap();
    let mut configured = child_event(pb::child_event::Kind::Ok(pb::Ok {}));
    configured.execution_clock = Some(pb::ExecutionClock::Cpu as i32);
    let ok = framed(&configured);
    let mut suspended = child_event(pb::child_event::Kind::FunctionCall(WireFunctionCall {
        function_name: "pause".to_owned(),
        ..WireFunctionCall::default()
    }));
    suspended.feed_execution_micros = reported;
    let suspend = framed(&suspended);
    let complete = framed(&child_event(pb::child_event::Kind::Complete(pb::Complete {
        values: Some(WireArena(vec![MontyNode::Int(1)].into())),
        value: 0,
    })));
    let print = framed(&child_event(pb::child_event::Kind::Print(pb::Print {
        segments: vec![pb::PrintSegment {
            stream: pb::PrintStream::Stdout as i32,
            text: "go\n".to_owned(),
        }]
        .into(),
    })));
    let script = format!(
        r"import os, signal, struct, sys, time
signal.alarm(15)
def read():
    header = sys.stdin.buffer.read(4)
    if len(header) != 4: sys.exit(0)
    size = struct.unpack('<I', header)[0]
    data = bytearray()
    while len(data) < size:
        chunk = sys.stdin.buffer.read(size - len(data))
        if not chunk: sys.exit(0)
        data.extend(chunk)
    return data
def send(data):
    sys.stdout.buffer.write(bytes(data))
    sys.stdout.buffer.flush()
def burn(ms):
    start = time.process_time_ns()
    while time.process_time_ns() - start < ms * 1000000: pass
open(os.path.join(os.path.dirname(__file__), 'ready'), 'w').close()
read()
send({ok:?})
count = 0
while True:
    read()
    count += 1
    {action}
"
    );
    let script = script
        .replace(
            "PARTIAL",
            &format!(
                "sys.stdout.buffer.write(bytes({:?})); sys.stdout.buffer.flush()",
                &complete[..2]
            ),
        )
        .replace("SUSPEND", &format!("send({suspend:?})"))
        .replace("COMPLETE", &format!("send({complete:?})"))
        .replace("PRINT", &format!("send({print:?})"));
    let program = dir.path().join("worker.py");
    fs::write(&program, script).unwrap();
    let quote = |path: &Path| format!("'{}'", path.display().to_string().replace('\'', "'\\''"));
    let launcher = format!("#!/bin/sh\nexec {} -I -S {} \"$@\"\n", quote(python), quote(&program));
    let binary = write_fake_monty(dir.path(), &launcher);
    (dir, supervised(binary))
}

async fn await_stand_in_ready(dir: &Path) {
    timeout(Duration::from_secs(5), async {
        while !dir.join("ready").exists() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("stand-in startup did not finish");
}

async fn stand_in_pool(mut config: PoolConfig, dir: &Path) -> Pool {
    config.min_processes = 1;
    let pool = Pool::new(config).await.unwrap();
    await_stand_in_ready(dir).await;
    pool
}

#[tokio::test]
async fn cpu_mode_requires_a_finite_wall_deadline() {
    for deadline in [None, Some(Duration::MAX)] {
        let mut config = supervised(monty_binary());
        config.request_timeout = deadline;
        assert!(matches!(Pool::new(config).await, Err(PoolError::Spawn(_))));
    }
}

#[tokio::test]
async fn healthy_native_worker_keeps_its_cooperative_limits() {
    let mut config = supervised(monty_binary());
    config.feed_duration_limit_grace = Some(Duration::from_millis(100));
    config.turn_duration_limit_grace = Some(Duration::from_millis(100));
    let pool = Pool::new(config).await.unwrap();
    let mut checkout = pool.checkout(&limits(50)).await.unwrap();
    assert_eq!(
        expect_complete(
            checkout
                .feed("sum(range(1000))", vec![], vec![], false, &mut no_print)
                .await
                .unwrap()
        ),
        MontyObject::int(499_500)
    );
    let error = checkout
        .feed("while True: pass", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    assert!(
        matches!(error, PoolError::Runtime(ref exc) if exc.exc_type() == ExcType::TimeoutError),
        "{error:?}"
    );
    assert_eq!(
        expect_complete(
            checkout
                .feed("1 + 1", vec![], vec![], false, &mut no_print)
                .await
                .unwrap()
        ),
        MontyObject::int(2)
    );
    drop(checkout);
    pool.close().await;
}

#[tokio::test]
async fn uncooperative_cpu_is_killed_and_reaped_for_typed_and_raw_turns() {
    for (budget_ms, raw, action) in [
        (50, false, "exec('while True: pass')"),
        (50, true, "exec('while True: pass')"),
        (50, true, "PARTIAL; exec('while True: pass')"),
        (100, false, "exec('while True: pass')"),
        (100, true, "exec('while True: pass')"),
        (100, true, "PARTIAL; exec('while True: pass')"),
    ] {
        let (dir, config) = stand_in(action, 0);
        let pool = stand_in_pool(config, dir.path()).await;
        let mut checkout = pool.checkout(&limits(budget_ms)).await.unwrap();
        let pid = checkout.pid().unwrap();
        let error = if raw {
            let request = pb::ParentRequest {
                kind: Some(pb::parent_request::Kind::Feed(pb::Feed::default())),
                ..pb::ParentRequest::default()
            };
            checkout
                .turn_raw(&request, &mut |_: &pb::ChildEvent| Box::pin(ready(())))
                .await
                .unwrap_err()
        } else {
            checkout
                .feed("ignored", vec![], vec![], false, &mut no_print)
                .await
                .unwrap_err()
        };
        assert!(
            matches!(error, PoolError::CpuLimit { limit, consumed } if limit == Duration::from_millis(budget_ms) && consumed >= limit),
            "{error:?}"
        );
        reaped(pid);
        assert!(matches!(
            checkout.feed("0", vec![], vec![], false, &mut no_print).await,
            Err(PoolError::Finished)
        ));
        drop(checkout);
        pool.close().await;
    }
}

#[tokio::test]
async fn blocked_child_is_killed_by_wall_time_instead_of_cpu() {
    let (dir, mut config) = stand_in("time.sleep(60)", 0);
    config.request_timeout = Some(Duration::from_millis(150));
    let pool = stand_in_pool(config, dir.path()).await;
    let mut checkout = pool.checkout(&limits(100)).await.unwrap();
    let pid = checkout.pid().unwrap();
    let error = checkout
        .feed("ignored", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    assert!(
        matches!(error, PoolError::Timeout { timeout } if timeout == Duration::from_millis(150)),
        "{error:?}"
    );
    reaped(pid);
    drop(checkout);
    pool.close().await;
}

#[tokio::test]
async fn worker_reports_cannot_spend_or_refund_the_supervisors_allowance() {
    for reported in [0, u64::MAX] {
        let (dir, mut config) = stand_in("burn(15); SUSPEND", reported);
        config.feed_duration_limit_grace = Some(Duration::from_millis(10));
        config.turn_duration_limit_grace = Some(Duration::from_millis(10));
        let pool = stand_in_pool(config, dir.path()).await;
        let mut checkout = pool.checkout(&limits(50)).await.unwrap();
        let pid = checkout.pid().unwrap();
        let mut event = checkout.feed("ignored", vec![], vec![], false, &mut no_print).await;
        let mut completed_turns = 0;
        while event.is_ok() {
            assert!(matches!(event, Ok(TurnEvent::FunctionCall { .. })));
            completed_turns += 1;
            assert!(completed_turns < 6, "suspensions reset the feed CPU allowance");
            // Host wait must not consume the child's CPU allowance.
            thread::sleep(Duration::from_millis(60));
            event = checkout
                .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
                .await;
        }
        assert!(
            completed_turns >= 2,
            "worker telemetry incorrectly spent the CPU budget"
        );
        assert!(matches!(event, Err(PoolError::CpuLimit { .. })), "{event:?}");
        reaped(pid);
        drop(checkout);
        pool.close().await;
    }
}

#[tokio::test]
async fn a_stopped_child_does_not_spend_cpu_while_it_waits_to_be_scheduled() {
    let (dir, config) = stand_in("burn(5); COMPLETE", 0);
    let pool = stand_in_pool(config, dir.path()).await;
    let mut checkout = pool.checkout(&limits(50)).await.unwrap();
    let pid = checkout.pid().unwrap();
    assert!(
        Command::new("kill")
            .args(["-STOP", &pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    let resume = async {
        sleep(Duration::from_millis(150)).await;
        assert!(
            Command::new("kill")
                .args(["-CONT", &pid.to_string()])
                .status()
                .unwrap()
                .success()
        );
    };
    let mut print = no_print;
    let (result, ()) = tokio::join!(checkout.feed("ignored", vec![], vec![], false, &mut print), resume);
    assert_eq!(expect_complete(result.unwrap()), MontyObject::int(1));
    drop(checkout);
    pool.close().await;
}

#[tokio::test]
async fn a_ready_reply_cannot_bypass_the_final_cpu_sample() {
    let (dir, config) = stand_in("PRINT; COMPLETE; burn(80)", 0);
    let pool = stand_in_pool(config, dir.path()).await;
    let mut checkout = pool.checkout(&limits(50)).await.unwrap();
    let pid = checkout.pid().unwrap();
    let mut callback = on_print_sync(|_, _| thread::sleep(Duration::from_millis(300)));
    let error = checkout
        .feed("ignored", vec![], vec![], false, &mut callback)
        .await
        .unwrap_err();
    assert!(matches!(error, PoolError::CpuLimit { .. }), "{error:?}");
    reaped(pid);
    drop(checkout);
    pool.close().await;
}

#[tokio::test]
async fn a_ready_reply_cannot_bypass_the_independent_wall_deadline() {
    let (dir, mut config) = stand_in("PRINT; COMPLETE", 0);
    config.request_timeout = Some(Duration::from_millis(100));
    let pool = stand_in_pool(config, dir.path()).await;
    let mut checkout = pool.checkout(&limits(50)).await.unwrap();
    let pid = checkout.pid().unwrap();
    let mut callback = on_print_sync(|_, _| thread::sleep(Duration::from_millis(200)));
    let error = checkout
        .feed("ignored", vec![], vec![], false, &mut callback)
        .await
        .unwrap_err();
    assert!(matches!(error, PoolError::Timeout { .. }), "{error:?}");
    reaped(pid);
    drop(checkout);
    pool.close().await;
}

#[tokio::test]
async fn native_idle_and_suspended_snapshots_restore_with_cpu_supervision() {
    let pool = Pool::new(supervised(monty_binary())).await.unwrap();
    for suspended in [false, true] {
        let mut source = pool.checkout(&limits(100)).await.unwrap();
        let code = if suspended {
            "from pathlib import Path\nPath('/snapshot-test.txt').read_text()"
        } else {
            "x = 42"
        };
        let event = source.feed(code, vec![], vec![], false, &mut no_print).await.unwrap();
        assert_eq!(matches!(event, TurnEvent::OsCall { .. }), suspended);
        let state = source.dump().await.unwrap();
        drop(source);
        let mut restored = pool.checkout(&limits(100)).await.unwrap();
        let (event, _) = restored.restore(state, vec![], &mut no_print).await.unwrap();
        if suspended {
            assert!(matches!(event, Some(TurnEvent::OsCall { .. })));
            let value = MontyObject::string("restored".to_owned());
            let event = restored
                .resume(ResumeValue::Return(value.clone()), &mut no_print)
                .await
                .unwrap();
            assert_eq!(expect_complete(event), value);
        } else {
            assert!(event.is_none());
            let event = restored.feed("x", vec![], vec![], false, &mut no_print).await.unwrap();
            assert_eq!(expect_complete(event), MontyObject::int(42));
        }
        restored.finish().await.unwrap();
    }
    pool.close().await;
}

#[tokio::test]
async fn restore_seeds_remaining_cpu_from_the_saved_vm_usage() {
    for raw in [false, true] {
        let (dir, config) = stand_in("if count == 1: SUSPEND\n    else: burn(25); COMPLETE", 35_000);
        let pool = stand_in_pool(config, dir.path()).await;
        let mut checkout = pool.checkout(&limits(50)).await.unwrap();
        let state = vec![0];
        if raw {
            let request = pb::ParentRequest {
                kind: Some(pb::parent_request::Kind::Load(pb::Load { state: state.into() })),
                ..pb::ParentRequest::default()
            };
            let event = checkout
                .turn_raw(&request, &mut |_: &pb::ChildEvent| Box::pin(ready(())))
                .await
                .unwrap();
            assert!(matches!(event.kind, Some(pb::child_event::Kind::FunctionCall(_))));
            let resume = pb::ParentRequest {
                kind: Some(pb::parent_request::Kind::ResumeCall(pb::ResumeCall::default())),
                ..pb::ParentRequest::default()
            };
            let error = checkout
                .turn_raw(&resume, &mut |_: &pb::ChildEvent| Box::pin(ready(())))
                .await
                .unwrap_err();
            assert!(
                matches!(error, PoolError::CpuLimit { limit, .. } if limit == Duration::from_millis(15)),
                "{error:?}"
            );
        } else {
            let (event, _) = checkout.restore(state, vec![], &mut no_print).await.unwrap();
            assert!(matches!(event, Some(TurnEvent::FunctionCall { .. })));
            let error = checkout
                .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
                .await
                .unwrap_err();
            assert!(
                matches!(error, PoolError::CpuLimit { limit, .. } if limit == Duration::from_millis(15)),
                "{error:?}"
            );
        }
        drop(checkout);
        pool.close().await;
    }
}

#[tokio::test]
async fn new_feeds_get_fresh_cpu_allowances() {
    let (dir, config) = stand_in("burn(15); COMPLETE", 0);
    let pool = stand_in_pool(config, dir.path()).await;
    let mut checkout = pool.checkout(&limits(50)).await.unwrap();
    for _ in 0..5 {
        assert_eq!(
            expect_complete(
                checkout
                    .feed("ignored", vec![], vec![], false, &mut no_print)
                    .await
                    .unwrap()
            ),
            MontyObject::int(1)
        );
    }
    drop(checkout);
    pool.close().await;
}

#[tokio::test]
async fn cancellation_discards_the_worker_before_any_resume() {
    let (dir, config) = stand_in("time.sleep(60)", 0);
    let pool = stand_in_pool(config, dir.path()).await;
    let mut checkout = pool.checkout(&limits(50)).await.unwrap();
    let pid = checkout.pid().unwrap();
    assert!(
        timeout(
            Duration::from_millis(30),
            checkout.feed("ignored", vec![], vec![], false, &mut no_print)
        )
        .await
        .is_err()
    );
    assert!(matches!(
        checkout.feed("ignored", vec![], vec![], false, &mut no_print).await,
        Err(PoolError::Protocol(_))
    ));
    timeout(Duration::from_secs(2), async {
        loop {
            #[cfg(target_os = "linux")]
            let alive = Path::new(&format!("/proc/{pid}")).exists();
            #[cfg(target_os = "macos")]
            let alive = Command::new("kill")
                .args(["-0", &pid.to_string()])
                .output()
                .unwrap()
                .status
                .success();
            if !alive {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("cancelled child was not reaped");
    reaped(pid);
    drop(checkout);
    pool.close().await;
}

#[tokio::test]
async fn a_child_exit_keeps_its_original_exit_status() {
    let (dir, config) = stand_in("sys.exit(64)", 0);
    let pool = stand_in_pool(config, dir.path()).await;
    let mut checkout = pool.checkout(&limits(50)).await.unwrap();
    let pid = checkout.pid().unwrap();
    let error = checkout
        .feed("ignored", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    assert!(
        matches!(error, PoolError::Crashed { status: Some(status), .. } if status.code() == Some(64)),
        "{error:?}"
    );
    reaped(pid);
    drop(checkout);
    pool.close().await;
}
#[tokio::test]
async fn a_raw_host_abort_allows_a_fresh_feed() {
    let exception = MontyException::new(ExcType::RuntimeError, Some("aborted".to_owned()));
    let error = framed(&child_event(pb::child_event::Kind::Error(pb::Error {
        exception: Some((&exception).into()),
    })));
    let action = format!("if count == 1: SUSPEND\n    elif count == 2: send({error:?})\n    else: COMPLETE");
    let (dir, config) = stand_in(&action, 0);
    let pool = stand_in_pool(config, dir.path()).await;
    let mut repl = limits(500);
    repl.limits.as_mut().unwrap().max_suspensions = 0;
    let mut checkout = pool.checkout(&repl).await.unwrap();
    let feed = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed::default())),
        ..pb::ParentRequest::default()
    };
    let mut on_event = |_: &pb::ChildEvent| -> PrintFuture { Box::pin(ready(())) };
    let event = checkout.turn_raw(&feed, &mut on_event).await.unwrap();
    assert!(matches!(event.kind, Some(pb::child_event::Kind::Error(_))));
    let event = checkout.turn_raw(&feed, &mut on_event).await.unwrap();
    assert!(matches!(event.kind, Some(pb::child_event::Kind::Complete(_))));
    let event = checkout
        .feed("ignored", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(1));
    drop(checkout);
    pool.close().await;
}

#[tokio::test]
async fn rejected_raw_requests_cannot_clear_a_native_suspension() {
    for rejected in [
        pb::parent_request::Kind::ResumeCall(pb::ResumeCall::default()),
        pb::parent_request::Kind::Load(pb::Load::default()),
    ] {
        let pool = Pool::new(supervised(monty_binary())).await.unwrap();
        let mut checkout = pool.checkout(&limits(500)).await.unwrap();
        let feed = pb::ParentRequest {
            kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
                code: "missing".to_owned(),
                cwd: "/".to_owned(),
                ..pb::Feed::default()
            })),
            ..pb::ParentRequest::default()
        };
        let mut on_event = |_: &pb::ChildEvent| -> PrintFuture { Box::pin(ready(())) };
        let event = checkout.turn_raw(&feed, &mut on_event).await.unwrap();
        assert!(matches!(event.kind, Some(pb::child_event::Kind::NameLookup(_))));
        let request = pb::ParentRequest {
            kind: Some(rejected),
            ..pb::ParentRequest::default()
        };
        let event = checkout.turn_raw(&request, &mut on_event).await.unwrap();
        assert!(matches!(event.kind, Some(pb::child_event::Kind::Error(_))));
        assert!(matches!(
            checkout.turn_raw(&feed, &mut on_event).await,
            Err(PoolError::Protocol(_))
        ));
        let resume = pb::ParentRequest {
            kind: Some(pb::parent_request::Kind::ResumeNameLookup(pb::ResumeNameLookup {
                kind: Some(pb::resume_name_lookup::Kind::Value(0)),
                values: Some(WireArena(vec![MontyNode::Int(42)].into())),
            })),
            ..pb::ParentRequest::default()
        };
        let event = checkout.turn_raw(&resume, &mut on_event).await.unwrap();
        assert!(matches!(event.kind, Some(pb::child_event::Kind::Complete(_))));
        let event = checkout
            .feed("1 + 1", vec![], vec![], false, &mut no_print)
            .await
            .unwrap();
        assert_eq!(expect_complete(event), MontyObject::int(2));
        checkout.finish().await.unwrap();
        pool.close().await;
    }
}

#[tokio::test]
async fn raw_suspensions_cannot_mint_allowance_through_an_invalid_feed() {
    let (dir, config) = stand_in("burn(15); SUSPEND", 0);
    let pool = stand_in_pool(config, dir.path()).await;
    let mut checkout = pool.checkout(&limits(50)).await.unwrap();
    let feed = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed::default())),
        ..pb::ParentRequest::default()
    };
    let resume = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
            call_id: 0,
            result: Some(pb::ExtFunctionResult {
                kind: Some(pb::ext_function_result::Kind::ReturnValue(0)),
            }),
            values: Some(WireArena(vec![MontyNode::None].into())),
        })),
        ..pb::ParentRequest::default()
    };
    let mut on_event = |_: &pb::ChildEvent| -> PrintFuture { Box::pin(ready(())) };
    let event = checkout.turn_raw(&feed, &mut on_event).await.unwrap();
    assert!(matches!(event.kind, Some(pb::child_event::Kind::FunctionCall(_))));
    let mut successful = 1;
    loop {
        assert!(matches!(
            checkout.turn_raw(&feed, &mut on_event).await,
            Err(PoolError::Protocol(_))
        ));
        assert!(matches!(
            checkout.feed("ignored", vec![], vec![], false, &mut no_print).await,
            Err(PoolError::Protocol(_))
        ));
        match checkout.turn_raw(&resume, &mut on_event).await {
            Ok(event) => {
                assert!(matches!(event.kind, Some(pb::child_event::Kind::FunctionCall(_))));
                successful += 1;
                assert!(successful <= 3, "invalid feeds reset the CPU ledger");
            }
            Err(PoolError::CpuLimit { .. }) => break,
            other => panic!("{other:?}"),
        }
    }
    drop(checkout);
    pool.close().await;
}

#[tokio::test]
async fn typed_control_errors_preserve_raw_suspension_accounting() {
    for operation in ["dump", "restore", "install"] {
        let pool = Pool::new(supervised(monty_binary())).await.unwrap();
        let mut repl = limits(1000);
        repl.type_check = operation == "dump";
        let mut checkout = pool.checkout(&repl).await.unwrap();
        let feed = pb::ParentRequest {
            kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
                code: if repl.type_check { "import tools" } else { "missing" }.to_owned(),
                cwd: "/".to_owned(),
                ..pb::Feed::default()
            })),
            ..pb::ParentRequest::default()
        };
        let mut on_event = |_: &pb::ChildEvent| -> PrintFuture { Box::pin(ready(())) };
        let event = checkout.turn_raw(&feed, &mut on_event).await.unwrap();
        assert!(matches!(
            event.kind,
            Some(pb::child_event::Kind::NameLookup(_) | pb::child_event::Kind::OsCall(_))
        ));
        let error = match operation {
            "dump" => checkout.dump().await.unwrap_err(),
            "restore" => checkout.restore(vec![], vec![], &mut no_print).await.unwrap_err(),
            "install" => checkout
                .install_dependencies(vec!["example-package".to_owned()])
                .await
                .unwrap_err(),
            _ => unreachable!(),
        };
        assert!(matches!(error, PoolError::Runtime(_)), "{error:?}");
        assert!(matches!(
            checkout.turn_raw(&feed, &mut on_event).await,
            Err(PoolError::Protocol(_))
        ));
        assert!(matches!(
            checkout.feed("0", vec![], vec![], false, &mut no_print).await,
            Err(PoolError::Protocol(_))
        ));
        checkout.finish().await.unwrap();
        pool.close().await;
    }
}

#[tokio::test]
async fn typed_dump_errors_do_not_refund_cumulative_cpu() {
    let exception = MontyException::new(ExcType::RuntimeError, Some("dump refused".to_owned()));
    let mut error = child_event(pb::child_event::Kind::Error(pb::Error {
        exception: Some((&exception).into()),
    }));
    // A control error cannot declare the suspended execution complete.
    error.feed_suspended = Some(false);
    let error = framed(&error);
    let action = format!("if count % 2 == 1: burn(15); SUSPEND\n    else: send({error:?})");
    let (dir, config) = stand_in(&action, 0);
    let pool = stand_in_pool(config, dir.path()).await;
    let mut checkout = pool.checkout(&limits(50)).await.unwrap();
    let pid = checkout.pid().unwrap();
    let event = checkout
        .feed("ignored", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert!(matches!(event, TurnEvent::FunctionCall { .. }));
    let feed = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed::default())),
        ..pb::ParentRequest::default()
    };
    let resume = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::ResumeCall(pb::ResumeCall::default())),
        ..pb::ParentRequest::default()
    };
    let mut on_event = |_: &pb::ChildEvent| -> PrintFuture { Box::pin(ready(())) };
    let mut completed = 1;
    loop {
        assert!(matches!(checkout.dump().await, Err(PoolError::Runtime(_))));
        assert!(matches!(
            checkout.turn_raw(&feed, &mut on_event).await,
            Err(PoolError::Protocol(_))
        ));
        match checkout.turn_raw(&resume, &mut on_event).await {
            Ok(event) => {
                assert!(matches!(event.kind, Some(pb::child_event::Kind::FunctionCall(_))));
                completed += 1;
                assert!(completed <= 3, "typed dump errors reset the CPU ledger");
            }
            Err(PoolError::CpuLimit { .. }) => break,
            other => panic!("{other:?}"),
        }
    }
    reaped(pid);
    drop(checkout);
    pool.close().await;
}

#[tokio::test]
async fn explicit_raw_abort_validates_the_exception_and_ends_the_feed() {
    let pool = Pool::new(supervised(monty_binary())).await.unwrap();
    let mut checkout = pool.checkout(&limits(500)).await.unwrap();
    let feed = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: "missing".to_owned(),
            cwd: "/".to_owned(),
            ..pb::Feed::default()
        })),
        ..pb::ParentRequest::default()
    };
    let mut on_event = |_: &pb::ChildEvent| -> PrintFuture { Box::pin(ready(())) };
    let event = checkout
        .feed("missing", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert!(matches!(event, TurnEvent::NameLookup { .. }));
    let exception = MontyException::new(ExcType::RuntimeError, Some("aborted by host".to_owned()));
    let valid = pb::RaisedException::from(&exception);
    let mut unknown_type = valid.clone();
    unknown_type.exc_type = "UnknownException".to_owned();
    let mut invalid_traceback = valid.clone();
    invalid_traceback.traceback = vec![pb::StackFrame::default()].into();
    for exception in [None, Some(unknown_type), Some(invalid_traceback)] {
        let abort = pb::ParentRequest {
            kind: Some(pb::parent_request::Kind::AbortFeed(pb::AbortFeed { exception })),
            ..pb::ParentRequest::default()
        };
        assert!(matches!(checkout.turn_raw(&abort, &mut on_event).await,
                Err(PoolError::Protocol(ref message)) if message == "AbortFeed requires a valid exception"));
        assert!(matches!(
            checkout.turn_raw(&feed, &mut on_event).await,
            Err(PoolError::Protocol(_))
        ));
    }
    let abort = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::AbortFeed(pb::AbortFeed {
            exception: Some(valid),
        })),
        ..pb::ParentRequest::default()
    };
    let event = checkout.turn_raw(&abort, &mut on_event).await.unwrap();
    let Some(pb::child_event::Kind::Error(error)) = event.kind else {
        panic!("{event:?}")
    };
    assert_eq!(
        MontyException::try_from(error.exception.unwrap()).unwrap().message(),
        Some("aborted by host")
    );
    let event = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(2));
    checkout.finish().await.unwrap();
    pool.close().await;
}

#[tokio::test]
async fn raw_errors_without_suspension_metadata_do_not_refund_cpu() {
    let exception = MontyException::new(ExcType::RuntimeError, Some("legacy error".to_owned()));
    let error = framed(&child_event(pb::child_event::Kind::Error(pb::Error {
        exception: Some((&exception).into()),
    })));
    let action = format!("if count % 2 == 1: burn(15); SUSPEND\n    else: send({error:?})");
    let (dir, config) = stand_in(&action, 0);
    let pool = stand_in_pool(config, dir.path()).await;
    let mut checkout = pool.checkout(&limits(50)).await.unwrap();
    let pid = checkout.pid().unwrap();
    let feed = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed::default())),
        ..pb::ParentRequest::default()
    };
    let resume = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::ResumeCall(pb::ResumeCall::default())),
        ..pb::ParentRequest::default()
    };
    let mut on_event = |_: &pb::ChildEvent| -> PrintFuture { Box::pin(ready(())) };
    checkout.turn_raw(&feed, &mut on_event).await.unwrap();
    let mut completed = 1;
    loop {
        let event = checkout.turn_raw(&resume, &mut on_event).await.unwrap();
        assert!(matches!(event.kind, Some(pb::child_event::Kind::Error(_))));
        assert_eq!(event.feed_suspended, None);
        assert!(matches!(
            checkout.turn_raw(&feed, &mut on_event).await,
            Err(PoolError::Protocol(_))
        ));
        match checkout.turn_raw(&resume, &mut on_event).await {
            Ok(event) => {
                assert!(matches!(event.kind, Some(pb::child_event::Kind::FunctionCall(_))));
                completed += 1;
                assert!(completed <= 3, "legacy errors reset the CPU ledger");
            }
            Err(PoolError::CpuLimit { .. }) => break,
            other => panic!("{other:?}"),
        }
    }
    reaped(pid);
    drop(checkout);
    pool.close().await;
}

#[tokio::test]
async fn raw_terminal_errors_clear_suspensions_in_both_modes() {
    for cpu in [false, true] {
        for raw_start in [false, true] {
            for code in ["f(); 1 / 0", "f(); raise RuntimeError('protocol violation: synthetic')"] {
                let mut config = supervised(monty_binary());
                config.execution_clock = if cpu { ExecutionClock::Cpu } else { ExecutionClock::Wall };
                let pool = Pool::new(config).await.unwrap();
                let repl = limits(1000);
                let mut checkout = pool.checkout(&repl).await.unwrap();
                let feed = |code: &str| pb::ParentRequest {
                    kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
                        code: code.to_owned(),
                        skip_type_check: true,
                        ..pb::Feed::default()
                    })),
                    ..pb::ParentRequest::default()
                };
                let mut on_event = |_: &pb::ChildEvent| -> PrintFuture { Box::pin(ready(())) };
                let call_id = if raw_start {
                    let event = checkout.turn_raw(&feed(code), &mut on_event).await.unwrap();
                    let Some(pb::child_event::Kind::FunctionCall(call)) = event.kind else {
                        panic!("{event:?}")
                    };
                    call.call_id
                } else {
                    let event = checkout.feed(code, vec![], vec![], false, &mut no_print).await.unwrap();
                    let TurnEvent::FunctionCall { call_id, .. } = event else {
                        panic!("{event:?}")
                    };
                    call_id
                };
                let invalid_abort = pb::ParentRequest {
                    kind: Some(pb::parent_request::Kind::AbortFeed(pb::AbortFeed::default())),
                    ..pb::ParentRequest::default()
                };
                assert!(matches!(
                    checkout.turn_raw(&invalid_abort, &mut on_event).await,
                    Err(PoolError::Protocol(_))
                ));
                let resume = |call_id| pb::ParentRequest {
                    kind: Some(pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
                        call_id,
                        result: Some(pb::ExtFunctionResult {
                            kind: Some(pb::ext_function_result::Kind::ReturnValue(0)),
                        }),
                        values: Some(WireArena(vec![MontyNode::None].into())),
                    })),
                    ..pb::ParentRequest::default()
                };
                let rejected = checkout.turn_raw(&resume(call_id + 1), &mut on_event).await.unwrap();
                assert!(matches!(rejected.kind, Some(pb::child_event::Kind::Error(_))));
                assert_eq!(rejected.feed_suspended, Some(true));
                let event = checkout.turn_raw(&resume(call_id), &mut on_event).await.unwrap();
                let Some(pb::child_event::Kind::Error(error)) = event.kind else {
                    panic!("{event:?}")
                };
                let error = MontyException::try_from(error.exception.unwrap()).unwrap();
                assert_eq!(
                    error.message(),
                    Some(if code.contains("1 / 0") {
                        "division by zero"
                    } else {
                        "protocol violation: synthetic"
                    })
                );
                assert_eq!(event.feed_suspended, Some(false));
                assert!(
                    matches!(checkout.resume(ResumeValue::Return(MontyObject::none()), &mut no_print).await,
                        Err(PoolError::Protocol(ref message)) if message == "no suspended call to resume")
                );
                let event = checkout
                    .feed("1 + 1", vec![], vec![], false, &mut no_print)
                    .await
                    .unwrap();
                assert_eq!(expect_complete(event), MontyObject::int(2));
                let event = checkout.turn_raw(&feed("2 + 2"), &mut on_event).await.unwrap();
                let Some(pb::child_event::Kind::Complete(complete)) = event.kind else {
                    panic!("{event:?}")
                };
                assert_eq!(MontyObject::try_from(complete).unwrap(), MontyObject::int(4));
                checkout.finish().await.unwrap();
                pool.close().await;
            }
        }
    }
}

#[tokio::test]
async fn invalid_typed_stub_answers_keep_the_feed_suspended() {
    let pool = Pool::new(supervised(monty_binary())).await.unwrap();
    let answers = [
        ResumeValue::Future,
        ResumeValue::Return(MontyObject::int(42)),
        ResumeValue::Return(MontyObject::dict([(MontyObject::int(1), MontyObject::none())])),
        ResumeValue::Return(MontyObject::dict([(MontyObject::string("other"), MontyObject::none())])),
        ResumeValue::Return(MontyObject::dict([(
            MontyObject::string("tools"),
            MontyObject::int(42),
        )])),
        ResumeValue::Return(MontyObject::dict([(
            MontyObject::string("tools..invalid"),
            MontyObject::string("value: int"),
        )])),
    ];
    for answer in answers {
        let mut repl = limits(1000);
        repl.type_check = true;
        let mut checkout = pool.checkout(&repl).await.unwrap();
        let event = checkout
            .feed("import tools", vec![], vec![], false, &mut no_print)
            .await
            .unwrap();
        assert_eq!(expect_import_stubs(event), vec!["tools".to_owned()]);
        assert!(matches!(
            checkout.resume(answer, &mut no_print).await,
            Err(PoolError::Protocol(_))
        ));
        let feed = pb::ParentRequest {
            kind: Some(pb::parent_request::Kind::Feed(pb::Feed::default())),
            ..pb::ParentRequest::default()
        };
        assert!(matches!(
            checkout
                .turn_raw(&feed, &mut |_: &pb::ChildEvent| Box::pin(ready(())))
                .await,
            Err(PoolError::Protocol(_))
        ));
        let exception = MontyException::new(ExcType::RuntimeError, Some("stub provider failed".to_owned()));
        let error = checkout
            .resume(ResumeValue::Error(exception), &mut no_print)
            .await
            .unwrap_err();
        assert!(matches!(error, PoolError::Runtime(ref exc) if exc.message() == Some("stub provider failed")));
        let event = checkout
            .feed("1 + 1", vec![], vec![], false, &mut no_print)
            .await
            .unwrap();
        assert_eq!(expect_complete(event), MontyObject::int(2));
        checkout.finish().await.unwrap();
    }
    pool.close().await;
}

#[tokio::test]
async fn raw_advances_cannot_leave_a_stale_typed_resume_token() {
    let pool = Pool::new(supervised(monty_binary())).await.unwrap();
    let mut checkout = pool.checkout(&limits(500)).await.unwrap();
    let event = checkout
        .feed("f(); f()", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    let TurnEvent::FunctionCall { call_id, .. } = event else {
        panic!("{event:?}")
    };
    // A non-advancing raw dump must leave the typed resume token usable.
    let dump = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Dump(pb::Dump::default())),
        ..pb::ParentRequest::default()
    };
    let mut on_event = |_: &pb::ChildEvent| -> PrintFuture { Box::pin(ready(())) };
    let event = checkout.turn_raw(&dump, &mut on_event).await.unwrap();
    assert!(matches!(event.kind, Some(pb::child_event::Kind::DumpResult(_))));
    let event = checkout
        .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
        .await
        .unwrap();
    let TurnEvent::FunctionCall { call_id: second, .. } = event else {
        panic!("{event:?}")
    };
    assert_ne!(call_id, second);
    let resume = |call_id| pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
            call_id,
            result: Some(pb::ExtFunctionResult {
                kind: Some(pb::ext_function_result::Kind::ReturnValue(0)),
            }),
            values: Some(WireArena(vec![MontyNode::None].into())),
        })),
        ..pb::ParentRequest::default()
    };
    let event = checkout.turn_raw(&resume(second), &mut on_event).await.unwrap();
    assert!(matches!(event.kind, Some(pb::child_event::Kind::Complete(_))));
    let event = checkout
        .feed("f(); f()", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    let TurnEvent::FunctionCall { call_id, .. } = event else {
        panic!("{event:?}")
    };
    let event = checkout.turn_raw(&resume(call_id), &mut on_event).await.unwrap();
    let Some(pb::child_event::Kind::FunctionCall(call)) = event.kind else {
        panic!("{event:?}")
    };
    assert_ne!(call.call_id, call_id);
    assert!(
        matches!(checkout.resume(ResumeValue::Return(MontyObject::none()), &mut no_print).await,
            Err(PoolError::Protocol(ref message)) if message == "no suspended call to resume")
    );
    let feed = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed::default())),
        ..pb::ParentRequest::default()
    };
    assert!(matches!(
        checkout.turn_raw(&feed, &mut on_event).await,
        Err(PoolError::Protocol(_))
    ));
    let event = checkout.turn_raw(&resume(call.call_id), &mut on_event).await.unwrap();
    assert!(matches!(event.kind, Some(pb::child_event::Kind::Complete(_))));
    let event = checkout
        .feed("1 + 1", vec![], vec![], false, &mut no_print)
        .await
        .unwrap();
    assert_eq!(expect_complete(event), MontyObject::int(2));
    checkout.finish().await.unwrap();
    pool.close().await;
}
