//! A single `monty --subprocess` child: spawn, framed I/O, and guaranteed
//! reaping.

use std::{
    env,
    process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};

use monty_proto::{FrameError, FrameReader, pb, write_frame};

use crate::{PoolConfig, PoolError};

/// A live child process with framed pipes.
///
/// The `Child` handle lives behind `Arc<Mutex<..>>` so the pool's watchdog
/// thread can kill the process while the owning thread is blocked reading
/// from it. Dropping a `Worker` always kills and reaps the child — no
/// zombies, no orphans.
pub(crate) struct Worker {
    /// Kill handle, shared with the watchdog.
    child: Arc<Mutex<Child>>,
    /// Child stdin; requests are written as frames via [`write_frame`].
    writer: ChildStdin,
    reader: FrameReader<ChildStdout>,
    /// Set by the watchdog just before it kills the child, so the read
    /// failure that follows is classified as a timeout rather than a crash.
    killed_for_timeout: Arc<AtomicBool>,
    /// Checkouts this worker has served, for `max_checkouts_per_worker`.
    pub(crate) checkouts_served: u32,
}

impl Worker {
    /// Spawns a child with framed pipes.
    ///
    /// There is no spawn-time handshake: a wrong or broken binary surfaces as
    /// an error on the first request the worker serves (typically the
    /// `Configure` of its first checkout).
    pub(crate) fn spawn(config: &PoolConfig) -> Result<Self, PoolError> {
        let mut command = Command::new(&config.binary_path);
        command
            .arg("--subprocess")
            // For extra safety, spawn the worker with an empty environment.
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        // Windows processes misbehave without SystemRoot (CRT and WinAPI
        // lookups); it names the OS install directory and is not sensitive.
        if cfg!(windows)
            && let Ok(system_root) = env::var("SystemRoot")
        {
            command.env("SystemRoot", system_root);
        }
        let mut child = command
            // stderr is inherited: child diagnostics stay visible to the host
            .spawn()
            .map_err(|err| PoolError::Spawn(format!("{}: {err}", config.binary_path.display())))?;

        let writer = child.stdin.take().expect("piped stdin");
        let reader = FrameReader::new(child.stdout.take().expect("piped stdout"));
        Ok(Self {
            child: Arc::new(Mutex::new(child)),
            writer,
            reader,
            killed_for_timeout: Arc::new(AtomicBool::new(false)),
            checkouts_served: 0,
        })
    }

    pub(crate) fn send(&mut self, request: &pb::ParentRequest) -> Result<(), FrameError> {
        write_frame(&mut self.writer, request)
    }

    /// Reads one event; EOF is an error here because within a checkout the
    /// child must never close its side first.
    pub(crate) fn recv(&mut self) -> Result<pb::ChildEvent, FrameError> {
        self.reader.read::<pb::ChildEvent>()?.ok_or(FrameError::Truncated)
    }

    pub(crate) fn pid(&self) -> u32 {
        lock_ignore_poison(&self.child).id()
    }

    /// Watchdog handles: the kill target and the timeout flag.
    pub(crate) fn kill_handles(&self) -> (Arc<Mutex<Child>>, Arc<AtomicBool>) {
        (Arc::clone(&self.child), Arc::clone(&self.killed_for_timeout))
    }

    /// Whether the watchdog killed this worker (consumes the flag's meaning:
    /// call once when classifying a read failure).
    pub(crate) fn was_killed_for_timeout(&self) -> bool {
        self.killed_for_timeout.load(Ordering::SeqCst)
    }

    /// Clears the sticky timeout flag at the start of a turn, scoping it to the
    /// currently-armed deadline. The watchdog sets the flag but never clears it,
    /// so without this reset a stale kill could misclassify the next turn's
    /// first I/O failure as a timeout.
    pub(crate) fn reset_killed_for_timeout(&self) {
        self.killed_for_timeout.store(false, Ordering::SeqCst);
    }

    /// Whether the child has already exited (used to discard workers that
    /// died while idle in the pool).
    pub(crate) fn is_dead(&self) -> bool {
        lock_ignore_poison(&self.child).try_wait().is_ok_and(|s| s.is_some())
    }

    /// Kills the child (best effort) and reaps it, returning the exit status
    /// when available.
    pub(crate) fn kill_and_reap(&mut self) -> Option<ExitStatus> {
        let mut child = lock_ignore_poison(&self.child);
        let _ = child.kill();
        child.wait().ok()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.kill_and_reap();
    }
}

/// Locks a possibly poisoned mutex; a panic elsewhere must not stop us from
/// killing/reaping children.
pub(crate) fn lock_ignore_poison<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
