//! Real process supervision on Linux, macOS and Windows, independent of interpreter polling.

#![cfg(any(target_os = "linux", target_os = "macos", windows))]

use std::{env, future::ready, path::PathBuf, sync::OnceLock, thread, time::Duration};

#[path = "binary/build.rs"]
mod build;

use monty_pool::{Pool, PoolConfig, PoolError, PrintFuture, ReplConfig, ResumeValue, TurnEvent};
use monty_types::{ExecutionClock, MontyObject, PrintStream, ResourceLimits};

fn no_print(_: PrintStream, _: &str) -> PrintFuture {
    Box::pin(ready(()))
}

fn worker() -> PathBuf {
    static WORKER: OnceLock<PathBuf> = OnceLock::new();

    // Prebuilt test bundles supply a fixture without a native Cargo installation.
    if let Some(path) = env::var_os("MONTY_CPU_TEST_BIN") {
        return PathBuf::from(path);
    }
    WORKER
        .get_or_init(|| build::binary(&["-p", "monty-pool", "--example", "cpu-test-worker"], "cpu-test-worker"))
        .clone()
}

fn config() -> PoolConfig {
    let worker = worker();
    let mut config = PoolConfig::subprocess(worker);
    config.execution_clock = ExecutionClock::Cpu;
    config.min_processes = 0;
    config.max_processes = 1;
    config.request_timeout = Some(Duration::from_secs(2));
    config.feed_duration_limit_grace = Some(Duration::ZERO);
    config.turn_duration_limit_grace = Some(Duration::ZERO);
    config
}

fn limits(budget: u64) -> ReplConfig {
    ReplConfig {
        limits: Some(
            ResourceLimits::default()
                .max_feed_duration(Duration::from_millis(budget))
                .max_turn_duration(Duration::from_millis(budget)),
        ),
        ..Default::default()
    }
}

#[tokio::test]
async fn healthy_child_and_unpolled_cpu_are_distinguished() {
    for budget in [50, 100] {
        let pool = Pool::new(config()).await.unwrap();
        let mut child = pool.checkout(&limits(budget)).await.unwrap();
        let event = child
            .feed("complete", vec![], vec![], false, &mut no_print)
            .await
            .unwrap();
        assert!(matches!(event, TurnEvent::Complete(ref result) if *result == MontyObject::int(42)));
        let error = child
            .feed("spin", vec![], vec![], false, &mut no_print)
            .await
            .unwrap_err();
        assert!(
            matches!(error, PoolError::CpuLimit { limit, consumed } if limit == Duration::from_millis(budget) && consumed >= limit),
            "{error:?}"
        );
        assert_eq!(child.pid(), None);
        assert!(matches!(
            child.feed("complete", vec![], vec![], false, &mut no_print).await,
            Err(PoolError::Finished)
        ));
        drop(child);
        pool.close().await;
    }
}

#[tokio::test]
async fn blocked_child_uses_the_independent_wall_deadline() {
    let pool = Pool::new(config()).await.unwrap();
    let mut child = pool.checkout(&limits(100)).await.unwrap();
    let error = child
        .feed("block", vec![], vec![], false, &mut no_print)
        .await
        .unwrap_err();
    assert!(
        matches!(error, PoolError::Timeout { timeout } if timeout == Duration::from_secs(2)),
        "{error:?}"
    );
    assert_eq!(child.pid(), None);
    drop(child);
    pool.close().await;
}

#[tokio::test]
async fn suspensions_and_zero_reports_cannot_refund_the_feed_budget() {
    for budget in [50, 100] {
        let pool = Pool::new(config()).await.unwrap();
        let mut child = pool.checkout(&limits(budget)).await.unwrap();
        let mut outcome = child.feed("pause", vec![], vec![], false, &mut no_print).await;
        let mut turns = 0;
        while let Ok(event) = outcome {
            assert!(matches!(event, TurnEvent::FunctionCall { .. }));
            turns += 1;
            assert!(turns < 30, "suspension renewed the feed allowance");
            thread::sleep(Duration::from_millis(30));
            outcome = child
                .resume(ResumeValue::Return(MontyObject::none()), &mut no_print)
                .await;
        }
        assert!(turns >= 2, "host wait was charged to the child");
        assert!(matches!(outcome, Err(PoolError::CpuLimit { .. })), "{outcome:?}");
        assert_eq!(child.pid(), None);
        drop(child);
        pool.close().await;
    }
}
