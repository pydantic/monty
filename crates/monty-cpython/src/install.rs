//! Installing third-party Python packages into a session's virtualenv with `uv`.
//!
//! A parent drives this via the `InstallDependencies` request (and the per-feed
//! PEP 723 auto-install): the child shells out to
//! `uv pip install --python <venv> <reqs>` to install into a virtualenv at
//! `./.venv` (relative to the worker's working directory), then makes that
//! venv's `site-packages` importable on the embedded interpreter by prepending
//! it to `sys.path` (so installs take precedence over the base environment) and
//! also passing it to `site.addsitedir` (so the venv's `.pth` files run — legacy
//! namespace packages rely on this).
//!
//! The venv is created once at image build time (`uv venv`, see the crate
//! `Dockerfile`), pinned to the same Python the worker embeds. It is a deployment
//! contract: if it is missing at install time the request fails with a clear
//! error rather than creating one on the fly (which, under a wrong working
//! directory or an unpinned interpreter, would silently land in the wrong place
//! or with a mismatched ABI). A worker serves exactly one session per process
//! inside a per-session sandbox, so the venv lives for that single session and is
//! reclaimed when the sandbox is torn down — nothing leaks into another session.
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
    io,
    path::{Path, PathBuf},
    process::Command,
};

use pyo3::{exceptions::PyRuntimeError, prelude::*};

/// Env var overriding the `uv` binary invoked for installs (default: `uv` on `PATH`).
const UV_ENV: &str = "MONTY_UV";

/// Cap on how much of uv's stderr is echoed back in a failure `Error`, in bytes.
const MAX_STDERR: usize = 8192;

/// A session's package-install location: the `./.venv` virtualenv into which uv
/// installs and whose `site-packages` is placed on the embedded interpreter's
/// `sys.path`.
///
/// The deployment (the image's `uv venv`) must have created the venv; an install
/// against a missing venv fails. It needs no explicit cleanup: the worker process
/// exits at the end of its single session and the per-session sandbox holding the
/// venv is discarded.
pub struct InstallEnv {
    /// The virtualenv directory (`./.venv` in the worker's working directory).
    venv: PathBuf,
    /// Whether the venv's `site-packages` has been added to `sys.path` yet.
    on_path: bool,
}

impl InstallEnv {
    /// Resolves the session virtualenv path: `.venv` in the worker's current
    /// working directory (the image sets the working dir and pre-creates it).
    pub fn create() -> io::Result<Self> {
        let venv = env::current_dir()?.join(".venv");
        Ok(Self { venv, on_path: false })
    }

    /// Installs `requirements` (PEP 508 strings) into the venv with `uv`, then
    /// makes the venv importable. The venv must already exist (the image creates
    /// it); a missing one is a deployment error, not something to paper over.
    /// Returns `Err(message)` carrying uv's stderr on a failed install, or a
    /// description of a missing venv / spawn / `sys.path` failure.
    pub fn install(&mut self, py: Python<'_>, requirements: &[String]) -> Result<(), String> {
        if !self.venv.is_dir() {
            return Err(format!(
                "no virtualenv at {}; the deployment must create it with `uv venv` (see the crate Dockerfile)",
                self.venv.display()
            ));
        }
        let uv = env::var_os(UV_ENV).unwrap_or_else(|| OsString::from("uv"));
        let mut cmd = Command::new(uv);
        cmd.arg("pip")
            .arg("install")
            .arg("--python")
            .arg(venv_python(&self.venv))
            .arg("--")
            .args(requirements);
        run_uv(cmd, "uv pip install")?;
        self.ensure_importable(py)
            .map_err(|err| format!("install succeeded but updating sys.path failed: {err}"))
    }

    /// Makes the venv's `site-packages` importable (once) and invalidates import
    /// caches so freshly installed packages are discoverable on the next import.
    ///
    /// The dir is both prepended to `sys.path` (so the session's installs take
    /// precedence over anything already on the embedded interpreter's path,
    /// rather than being shadowed by it) *and* passed to `site.addsitedir` (so
    /// the venv's `.pth` files run — legacy namespace packages rely on this).
    /// Because the front insert happens first, `addsitedir` sees the dir already
    /// on `sys.path` and does not append a second, lower-priority copy.
    fn ensure_importable(&mut self, py: Python<'_>) -> PyResult<()> {
        if !self.on_path {
            let version = interpreter_version(py)
                .ok_or_else(|| PyRuntimeError::new_err("could not read the embedded interpreter version"))?;
            let site_packages = venv_site_packages(&self.venv, &version).to_string_lossy().into_owned();
            py.import("sys")?
                .getattr("path")?
                .call_method1("insert", (0, &site_packages))?;
            py.import("site")?.call_method1("addsitedir", (site_packages,))?;
            self.on_path = true;
        }
        py.import("importlib")?.call_method0("invalidate_caches")?;
        Ok(())
    }
}

/// Runs a prepared `uv` command, mapping a spawn failure or non-zero exit (with
/// uv's truncated stderr) onto an `Err(message)`. `action` names the step for the
/// error text (e.g. `"uv pip install"`).
fn run_uv(mut cmd: Command, action: &str) -> Result<(), String> {
    let output = cmd.output().map_err(|err| {
        format!(
            "failed to run {action}: {err}; ensure uv is installed and on PATH, or set {UV_ENV} to its absolute path"
        )
    })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{action} failed: {}",
            truncate(&String::from_utf8_lossy(&output.stderr))
        ))
    }
}

/// The venv's Python executable: `bin/python`, or `Scripts\python.exe` on Windows.
fn venv_python(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python")
    }
}

/// The venv's `site-packages` directory for interpreter version `X.Y`:
/// `lib/pythonX.Y/site-packages`, or `Lib\site-packages` on Windows.
fn venv_site_packages(venv: &Path, version: &str) -> PathBuf {
    if cfg!(windows) {
        venv.join("Lib").join("site-packages")
    } else {
        venv.join("lib").join(format!("python{version}")).join("site-packages")
    }
}

/// The embedded interpreter's `major.minor` version (e.g. `"3.14"`), used to
/// locate the venv's `site-packages` (`lib/pythonX.Y/...`). `None` if unreadable.
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
