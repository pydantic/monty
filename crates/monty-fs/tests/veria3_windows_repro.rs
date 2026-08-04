//! VERIA-3: on Windows, `PathBuf::join` discards the mount base when a
//! `/`-split segment carries a drive/UNC/root prefix (e.g. `C:\Windows\x`,
//! `\\host\share\x`), so an untrusted virtual path escapes the mount.
//!
//! All three original vectors share that one root cause, so they are folded
//! into a single parameterized test asserting the invariant: an out-of-mount
//! candidate must be rejected with `PathEscape` — specifically, not just any
//! error, since an `Io` error means the layer already did the escaping I/O
//! (for UNC, the credential-leaking SMB handshake). Windows-only.
//!
//!   cargo test -p monty-fs --test veria3_windows_repro -- --nocapture
//!
//! The UNC payload uses an RFC-2606 `.invalid` host, so even the vulnerable
//! build reaches no real host and leaks nothing while proving the escape.

use std::{
    fs,
    path::{Path, PathBuf},
};

use monty_fs::{MountCallOutcome, MountError, MountMode, MountTable};
use monty_types::{MkdirCallArgs, MontyObject, OsFunctionCall};
use tempfile::TempDir;

/// Mounts `host` at `/mnt` in `ReadWrite` (the mode whose writes hit the host).
fn mount_rw(host: &Path) -> MountTable {
    let mut mt = MountTable::new();
    mt.mount("/mnt", host, MountMode::ReadWrite, None)
        .expect("failed to configure mount");
    mt
}

/// Dispatches a call, panicking if the mount table declines it (which would
/// mean the attack string never reached the boundary — not what we test).
fn dispatch(mt: &mut MountTable, call: OsFunctionCall) -> Result<MontyObject, MountError> {
    match mt.handle_os_call(call) {
        MountCallOutcome::Handled(result) => result,
        MountCallOutcome::NotHandled(call) => {
            panic!("mount table did not handle the call (returned NotHandled): {call:?}")
        }
    }
}

/// The write vector (`os.makedirs(parents=True)`) or the read/oracle vector.
enum Probe {
    Exists,
    MkdirParents,
}

/// One out-of-mount payload and the operation that must reject it. Owns its
/// temp dirs so they outlive the dispatch.
struct EscapeCase {
    /// Label used in the log line and failure list.
    name: &'static str,
    /// Virtual path fed to the sandbox (`/mnt/<out-of-mount tail>`).
    virtual_path: String,
    probe: Probe,
    mount: TempDir,
    _outside: Option<TempDir>,
    /// For write vectors, the host path that must NOT exist afterward.
    side_effect_target: Option<PathBuf>,
}

/// Builds the three VERIA-3 vectors as cases sharing one invariant.
fn escape_cases() -> Vec<EscapeCase> {
    // Drive-letter write of an absolute path under a SIBLING dir (outside the
    // mount): vulnerable build creates it, secure build returns `PathEscape`.
    let write = {
        let mount = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("veria3_pwned");
        assert!(!target.exists(), "precondition: write target must not pre-exist");
        let segment = target.to_str().expect("temp path is valid UTF-8");
        EscapeCase {
            name: "drive-letter-write",
            virtual_path: format!("/mnt/{segment}"),
            probe: Probe::MkdirParents,
            mount,
            _outside: Some(outside),
            side_effect_target: Some(target),
        }
    };

    // Existence oracle: `exists` of a MISSING out-of-mount path. Vulnerable
    // build returns `Ok(false)` (distinguishable from an existing one's
    // `PathEscape` — the leak); secure build returns `PathEscape` for both.
    let oracle = {
        let mount = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let missing = outside.path().join("definitely_absent_xyz");
        let segment = missing.to_str().expect("temp path is valid UTF-8");
        EscapeCase {
            name: "existence-oracle",
            virtual_path: format!("/mnt/{segment}"),
            probe: Probe::Exists,
            mount,
            _outside: Some(outside),
            side_effect_target: None,
        }
    };

    // UNC egress: vulnerable build resolves the path (an SMB/NTLM handshake in
    // the wild); secure build rejects it before any resolution.
    let egress = EscapeCase {
        name: "unc-egress",
        virtual_path: r"/mnt/\\monty-veria3.invalid\share\probe".to_owned(),
        probe: Probe::Exists,
        mount: TempDir::new().unwrap(),
        _outside: None,
        side_effect_target: None,
    };

    vec![write, oracle, egress]
}

/// Every out-of-mount candidate must be rejected with `PathEscape` before any
/// I/O. Runs all branches and fails listing any that escaped.
#[test]
#[cfg_attr(not(windows), ignore = "VERIA-3 is a Windows-only path-parsing escape")]
fn out_of_mount_candidates_are_rejected() {
    let cases = escape_cases();
    let mut escaped: Vec<&str> = Vec::new();

    for case in &cases {
        let call = match case.probe {
            Probe::Exists => OsFunctionCall::Exists(case.virtual_path.clone().into()),
            Probe::MkdirParents => OsFunctionCall::Mkdir(MkdirCallArgs {
                path: case.virtual_path.clone().into(),
                parents: true,
                exist_ok: false,
            }),
        };

        let mut mt = mount_rw(case.mount.path());
        let outcome = dispatch(&mut mt, call);

        // A write vector also escapes if it left a directory behind, even on error.
        let side_effect = case.side_effect_target.as_ref().is_some_and(|t| t.exists());
        if let Some(target) = &case.side_effect_target {
            let _ = fs::remove_dir_all(target); // best-effort cleanup if owned
        }

        let rejected = matches!(outcome, Err(MountError::PathEscape { .. })) && !side_effect;
        println!(
            "[{}] path={} outcome={outcome:?} side_effect={side_effect} => {}",
            case.name,
            case.virtual_path,
            if rejected { "REJECTED" } else { "ESCAPED" },
        );
        if !rejected {
            escaped.push(case.name);
        }
    }

    assert!(
        escaped.is_empty(),
        "SANDBOX ESCAPE: out-of-mount candidates were not rejected with PathEscape \
         before I/O on branches: {escaped:?}"
    );
}
