//! Helpers shared by the pool integration test crates.

use std::time::Duration;
#[cfg(unix)]
use std::{
    path::{Path, PathBuf},
    process::Command,
};

use monty_pool::Pool;
use tokio::time::{Instant, sleep};

/// Polls until the pool holds exactly `count` idle workers, panicking after a
/// few seconds. Background refills land asynchronously, so tests wait rather
/// than sleeping a fixed time.
pub async fn wait_for_idle(pool: &Pool, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while pool.idle_workers() != count {
        assert!(
            Instant::now() < deadline,
            "idle workers stuck at {}, expected {count}",
            pool.idle_workers()
        );
        sleep(Duration::from_millis(10)).await;
    }
}

/// Writes the executable stand-in for `monty` that a test drives, from a child shell rather than
/// this process: a descriptor open for writing here would be inherited by any concurrently
/// forking test, and executing the file while that child still holds it fails with `ETXTBSY`.
#[cfg(unix)]
pub fn write_fake_monty(dir: &Path, script: &str) -> PathBuf {
    let fake = dir.join("monty");
    let status = Command::new("sh")
        .args(["-c", r#"printf '%s' "$2" > "$1" && chmod 755 "$1""#, "sh"])
        .arg(&fake)
        .arg(script)
        .status()
        .unwrap();
    assert!(status.success(), "writing the stand-in `monty` failed");
    fake
}
