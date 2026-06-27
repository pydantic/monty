//! Installing third-party Python packages into a session with `uv`.
//!
//! A parent drives this via the `InstallDependencies` request: the child shells
//! out to `uv pip install --target <dir>` and then adds `<dir>` to the embedded
//! interpreter's `sys.path` so subsequent feeds can import the packages. The
//! target dir is per-session and lives under the OS temp dir; it is removed from
//! disk when the [`InstallEnv`] is dropped (on `Reset`/teardown) and unlinked
//! from `sys.path` via [`InstallEnv::remove_from_path`].
//!
//! `uv` is expected on `PATH` (the deployment's Docker image installs it),
//! overridable with the `MONTY_UV` env var for non-standard images.
//!
//! SECURITY: this shells out to `uv`, which reaches the network to fetch
//! packages and writes them to the host filesystem. It is only ever reached by
//! the embedded-CPython worker, which is explicitly **not** a sandbox (see the
//! crate `README.md`); the Monty sandbox child rejects `InstallDependencies`.

use std::{
    env,
    ffi::OsString,
    fs, io,
    path::PathBuf,
    process::{Command, id},
    sync::atomic::{AtomicU64, Ordering},
};

use pyo3::prelude::*;

/// Env var overriding the `uv` binary invoked for installs (default: `uv` on `PATH`).
const UV_ENV: &str = "MONTY_UV";

/// Cap on how much of uv's stderr is echoed back in a failure `Error`, in bytes.
const MAX_STDERR: usize = 8192;

/// Process-unique counter feeding per-session install-dir names, so a worker
/// that serves several sessions (across `Reset`) never reuses a directory.
static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A session's package-install location: a `uv pip install --target` directory
/// that is also placed on the interpreter's `sys.path`.
///
/// Created lazily on the first non-empty install and torn down with the session:
/// `Drop` removes the directory from disk, and [`Self::remove_from_path`] (which
/// needs the interpreter) unlinks it from `sys.path` first.
pub struct InstallEnv {
    /// The `--target` directory uv installs into; also added to `sys.path`.
    target: PathBuf,
    /// Whether `target` has already been pushed onto `sys.path`.
    on_path: bool,
}

impl InstallEnv {
    /// Creates a fresh, empty per-session install directory under the OS temp dir.
    pub fn create() -> io::Result<Self> {
        let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut target = env::temp_dir();
        target.push(format!("monty-cpython-deps-{}-{n}", id()));
        fs::create_dir_all(&target)?;
        Ok(Self { target, on_path: false })
    }

    /// Installs `requirements` (PEP 508 strings) with `uv`, then ensures the
    /// target dir is importable. Returns `Err(message)` carrying uv's stderr on
    /// a failed install, or a description of any spawn/`sys.path` failure.
    pub fn install(&mut self, py: Python<'_>, requirements: &[String]) -> Result<(), String> {
        let uv = env::var_os(UV_ENV).unwrap_or_else(|| OsString::from("uv"));
        let mut cmd = Command::new(&uv);
        cmd.arg("pip").arg("install").arg("--target").arg(&self.target);
        // Pin resolution to the embedded interpreter's Python version so uv picks
        // wheels with a matching ABI tag (which is minor-version based). We pass
        // the bare `X.Y` rather than `sys.executable` because, in an embedded
        // runtime, `sys.executable` is this worker binary, not a Python uv can
        // query; `X.Y` lets uv locate (or fetch) a matching real interpreter.
        if let Some(version) = interpreter_version(py) {
            cmd.arg("--python").arg(version);
        }
        cmd.args(requirements);

        let output = cmd.output().map_err(|err| {
            format!(
                "failed to run uv ({}): {err}; ensure uv is installed and on PATH, or set {UV_ENV} to its absolute path",
                uv.to_string_lossy()
            )
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("uv pip install failed: {}", truncate(&stderr)));
        }
        self.ensure_importable(py)
            .map_err(|err| format!("install succeeded but updating sys.path failed: {err}"))
    }

    /// Adds the target dir to `sys.path` (once) and invalidates import caches so
    /// freshly installed packages are discoverable on the next import.
    fn ensure_importable(&mut self, py: Python<'_>) -> PyResult<()> {
        if !self.on_path {
            let target = self.target.to_string_lossy().into_owned();
            py.import("sys")?.getattr("path")?.call_method1("insert", (0, target))?;
            self.on_path = true;
        }
        py.import("importlib")?.call_method0("invalidate_caches")?;
        Ok(())
    }

    /// Removes the target dir from `sys.path` so a later session in the same
    /// worker does not see this session's packages. Best-effort: a missing entry
    /// (never installed into) is ignored. Call before dropping, while the
    /// interpreter is still available.
    pub fn remove_from_path(&self, py: Python<'_>) {
        if !self.on_path {
            return;
        }
        let target = self.target.to_string_lossy().into_owned();
        // list.remove raises ValueError if absent; ignore either way.
        if let Ok(sys) = py.import("sys")
            && let Ok(path) = sys.getattr("path")
        {
            let _ = path.call_method1("remove", (target,));
        }
    }
}

impl Drop for InstallEnv {
    /// Removes the install directory from disk. `sys.path` is unlinked separately
    /// by [`Self::remove_from_path`], which needs the interpreter `Drop` lacks.
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.target);
    }
}

/// The embedded interpreter's `major.minor` version (e.g. `"3.14"`) for uv's
/// `--python` request, or `None` if it cannot be read.
fn interpreter_version(py: Python<'_>) -> Option<String> {
    let info = py.import("sys").ok()?.getattr("version_info").ok()?;
    let major: u8 = info.getattr("major").ok()?.extract().ok()?;
    let minor: u8 = info.getattr("minor").ok()?.extract().ok()?;
    Some(format!("{major}.{minor}"))
}

/// Caps `s` at [`MAX_STDERR`] bytes (on a char boundary), marking truncation.
fn truncate(s: &str) -> String {
    if s.len() <= MAX_STDERR {
        return s.to_owned();
    }
    let end = (0..=MAX_STDERR).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0);
    format!("{}… (truncated)", &s[..end])
}
