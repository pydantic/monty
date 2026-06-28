//! Installing third-party Python packages into a session with `uv`.
//!
//! A parent drives this via the `InstallDependencies` request: the child shells
//! out to `uv pip install --target <dir>` and then adds `<dir>` to the embedded
//! interpreter's `sys.path` so subsequent feeds can import the packages. The
//! target dir lives under the OS temp dir and is removed from disk when the
//! [`InstallEnv`] (a [`TempDir`]) is dropped at session teardown.
//!
//! A worker serves exactly one session per process, so there is one install dir
//! per process: its `sys.path` entry and any imported modules die with the
//! process and never leak into another session. The dir is created with
//! [`TempDir`], i.e. a unique random name created exclusively (an attacker
//! cannot pre-create or reuse a predictable path).
//!
//! `uv` is expected on `PATH` (the deployment's Docker image installs it),
//! overridable with the `MONTY_UV` env var for non-standard images.
//!
//! SECURITY: this shells out to `uv`, which reaches the network to fetch
//! packages and writes them to the host filesystem. It is only ever reached by
//! the embedded-CPython worker, which is explicitly **not** a sandbox (see the
//! crate `README.md`); the Monty sandbox child rejects `InstallDependencies`.

use std::{env, ffi::OsString, io, process::Command};

use pyo3::prelude::*;
use tempfile::{Builder, TempDir};

/// Env var overriding the `uv` binary invoked for installs (default: `uv` on `PATH`).
const UV_ENV: &str = "MONTY_UV";

/// Cap on how much of uv's stderr is echoed back in a failure `Error`, in bytes.
const MAX_STDERR: usize = 8192;

/// A session's package-install location: a `uv pip install --target` directory
/// that is also placed on the interpreter's `sys.path`.
///
/// Created lazily on the first non-empty install and torn down with the session:
/// dropping the [`TempDir`] removes the directory from disk. The `sys.path`
/// entry needs no explicit cleanup because the worker process exits at the end
/// of its single session.
pub struct InstallEnv {
    /// The `--target` directory uv installs into; also added to `sys.path`. A
    /// uniquely named temp dir, removed from disk when this `TempDir` drops.
    dir: TempDir,
    /// Whether the dir has already been pushed onto `sys.path`.
    on_path: bool,
}

impl InstallEnv {
    /// Creates a fresh, uniquely named install directory under the OS temp dir.
    pub fn create() -> io::Result<Self> {
        let dir = Builder::new().prefix("monty-cpython-deps-").tempdir()?;
        Ok(Self { dir, on_path: false })
    }

    /// Installs `requirements` (PEP 508 strings) with `uv`, then ensures the
    /// target dir is importable. Returns `Err(message)` carrying uv's stderr on
    /// a failed install, or a description of any spawn/`sys.path` failure.
    pub fn install(&mut self, py: Python<'_>, requirements: &[String]) -> Result<(), String> {
        let uv = env::var_os(UV_ENV).unwrap_or_else(|| OsString::from("uv"));
        let mut cmd = Command::new(&uv);
        cmd.arg("pip").arg("install").arg("--target").arg(self.dir.path());
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
            let target = self.dir.path().to_string_lossy().into_owned();
            py.import("sys")?.getattr("path")?.call_method1("insert", (0, target))?;
            self.on_path = true;
        }
        py.import("importlib")?.call_method0("invalidate_caches")?;
        Ok(())
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
