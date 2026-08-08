//! Confinement guarantees that hold regardless of what happens to the host
//! directory *after* the mount is created.
//!
//! Only `mount_root_swapped_for_symlink_still_reads_original` and
//! `mount_survives_host_directory_rename` distinguish this design from
//! path-based resolution — verified red against it, on Unix; Windows refuses to
//! rename a mounted directory at all, so it gets its own pair. The other escape
//! tests pass under both, so they are regression guards, not evidence, and the
//! two timed races are soaks: neither was ever won on macOS/APFS, so a green
//! run proves little on any one platform. Those two skip unless `MONTY_FS_SOAK`
//! is set (see [`soak_enabled`]); everything else runs by default.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    env, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use common::{symlink_dir, symlink_file, symlinks_supported, try_rename_mount_root};
use monty_fs::{MountCallOutcome, MountError, MountMode, MountTable, OverlayState};
#[cfg(unix)]
use monty_types::{FileMode, OpenCallArgs};
use monty_types::{MkdirCallArgs, MontyObject, OsFunctionCall, PathStringDataArgs};
#[cfg(unix)]
use nix::{sys::stat::Mode, unistd::mkfifo};
use tempfile::TempDir;

mod common;

/// Marker written to the out-of-mount file; its appearance in any result is the
/// disclosure these tests guard against.
const SECRET: &str = "SECRET-HOST-CONTENT";

/// Whether the multi-second race soaks in this file should run.
///
/// They burn ~5s of wall clock to prove very little: neither race was ever won
/// on macOS/APFS, so a green run is a soak, not evidence. Gating them keeps a
/// local `cargo test -p monty-fs` fast; CI runs them on every OS with
/// `MONTY_FS_SOAK=1`, as should anyone changing the paths they cover.
fn soak_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        let enabled = env::var_os("MONTY_FS_SOAK").is_some_and(|value| !value.is_empty() && value != "0");
        if !enabled {
            eprintln!("race soaks will skip: set MONTY_FS_SOAK=1 to run them");
        }
        enabled
    })
}

/// Dispatches a call, panicking if the mount table declines to handle it.
fn dispatch(mounts: &mut MountTable, call: OsFunctionCall) -> Result<MontyObject, MountError> {
    match mounts.handle_os_call(call) {
        MountCallOutcome::Handled(result) => result,
        MountCallOutcome::NotHandled(call) => panic!("mount table returned NotHandled: {call:?}"),
    }
}

fn read_text(mounts: &mut MountTable, path: &str) -> Result<MontyObject, MountError> {
    dispatch(mounts, OsFunctionCall::ReadText(path.into()))
}

fn mount_rw(host: &Path) -> MountTable {
    let mut mounts = MountTable::new();
    mounts
        .mount("/mnt", host, MountMode::ReadWrite, None)
        .expect("failed to configure mount");
    mounts
}

/// The mount is pinned at mount time, so replacing the host directory with a
/// symlink elsewhere must not redirect reads to it.
///
/// Deterministic, and the two designs differ observably: resolving by path
/// re-reads the swapped root on every call, whereas a descriptor opened at
/// mount time still refers to the original directory.
#[test]
fn mount_root_swapped_for_symlink_still_reads_original() {
    if !symlinks_supported() {
        return;
    }
    let base = TempDir::new().unwrap();
    let mount_path = base.path().join("mount");
    let elsewhere = base.path().join("elsewhere");
    fs::create_dir(&mount_path).unwrap();
    fs::create_dir(&elsewhere).unwrap();
    fs::write(mount_path.join("file.txt"), "public").unwrap();
    fs::write(elsewhere.join("file.txt"), SECRET).unwrap();

    let mut mounts = mount_rw(&mount_path);

    // Replace the mount root with a symlink pointing somewhere else entirely.
    if !try_rename_mount_root(&mount_path, base.path().join("moved")) {
        // Windows blocks the rename outright, so the swap cannot even be staged
        // — a stronger guarantee than the one asserted below.
        assert!(matches!(read_text(&mut mounts, "/mnt/file.txt"), Ok(MontyObject::String(s)) if s == "public"));
        return;
    }
    symlink_dir(&elsewhere, &mount_path);

    let outcome = read_text(&mut mounts, "/mnt/file.txt");
    let leaked = matches!(&outcome, Ok(MontyObject::String(s)) if s.contains(SECRET));
    assert!(!leaked, "HOST FILE DISCLOSURE: read followed the swapped mount root");
    assert!(
        matches!(&outcome, Ok(MontyObject::String(s)) if s == "public"),
        "expected the original directory to still back the mount, got {outcome:?}"
    );
}

/// Renaming the mount's host directory must not detach the mount.
///
/// A descriptor follows the directory through a rename; a stored path does not.
/// Unix-only: Windows refuses the rename outright, covered below.
#[test]
#[cfg(unix)]
fn mount_survives_host_directory_rename() {
    let base = TempDir::new().unwrap();
    let mount_path = base.path().join("mount");
    fs::create_dir(&mount_path).unwrap();
    fs::write(mount_path.join("file.txt"), "public").unwrap();

    let mut mounts = mount_rw(&mount_path);
    fs::rename(&mount_path, base.path().join("renamed")).unwrap();

    let outcome = read_text(&mut mounts, "/mnt/file.txt");
    assert!(
        matches!(&outcome, Ok(MontyObject::String(s)) if s == "public"),
        "expected the mount to follow its directory through a rename, got {outcome:?}"
    );
}

/// On Windows the mount's open handle blocks the rename entirely, so the host
/// cannot move or delete a mounted directory until the mount is dropped.
#[test]
#[cfg(windows)]
fn mounted_directory_cannot_be_renamed_on_windows() {
    let base = TempDir::new().unwrap();
    let mount_path = base.path().join("mount");
    fs::create_dir(&mount_path).unwrap();
    fs::write(mount_path.join("file.txt"), "public").unwrap();

    let mut mounts = mount_rw(&mount_path);
    let err = fs::rename(&mount_path, base.path().join("renamed")).unwrap_err();
    assert_eq!(err.raw_os_error(), Some(32), "expected ERROR_SHARING_VIOLATION");

    let outcome = read_text(&mut mounts, "/mnt/file.txt");
    assert!(matches!(&outcome, Ok(MontyObject::String(s)) if s == "public"));
}

/// An intermediate directory replaced by a symlink out of the mount must not
/// become a route out, whether or not any read is in flight.
///
/// Not a design differentiator on its own — resolving by path also rejects this,
/// via a boundary check rather than structurally.
#[test]
fn read_through_swapped_intermediate_directory_is_rejected() {
    if !symlinks_supported() {
        return;
    }
    let mount_dir = TempDir::new().unwrap();
    let outside_dir = TempDir::new().unwrap();
    fs::write(outside_dir.path().join("secret.txt"), SECRET).unwrap();

    let data = mount_dir.path().join("data");
    fs::create_dir(&data).unwrap();
    fs::write(data.join("file.txt"), "public").unwrap();

    let mut mounts = mount_rw(mount_dir.path());

    fs::remove_dir_all(&data).unwrap();
    symlink_dir(outside_dir.path(), &data);

    let outcome = read_text(&mut mounts, "/mnt/data/secret.txt");
    let leaked = matches!(&outcome, Ok(MontyObject::String(s)) if s.contains(SECRET));
    assert!(!leaked, "HOST FILE DISCLOSURE: read traversed an outbound symlink");
}

/// The reported vector: one session renaming while another reads the same path.
///
/// The swap alternates `/mnt/data/file.txt` between a real in-mount file and a
/// symlink to an out-of-mount file, so a read whose check and open disagree
/// returns the secret. A descriptor-relative open has no window to lose — but
/// this only ever *detects* the old failure when the race is won, which did not
/// happen on macOS/APFS, so treat a green run as a soak rather than a proof —
/// hence [`soak_enabled`].
#[test]
fn concurrent_rename_cannot_redirect_a_read() {
    if !symlinks_supported() || !soak_enabled() {
        return;
    }
    let mount_dir = TempDir::new().unwrap();
    let outside_dir = TempDir::new().unwrap();

    let secret = outside_dir.path().join("secret.txt");
    fs::write(&secret, SECRET).unwrap();

    let data = mount_dir.path().join("data");
    fs::create_dir(&data).unwrap();
    let target = data.join("file.txt");
    fs::write(&target, "public").unwrap();

    // The pre-existing outbound symlink: sandboxed code can relocate it but
    // never create one.
    let decoy = mount_dir.path().join("decoy");
    symlink_file(&secret, &decoy);

    let mut mounts = mount_rw(mount_dir.path());
    let stop = Arc::new(AtomicBool::new(false));
    let swaps = Arc::new(AtomicU32::new(0));

    let swapper = {
        let (stop, swaps) = (Arc::clone(&stop), Arc::clone(&swaps));
        let staging = mount_dir.path().join("staging");
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                // Restore the known-good arrangement first. The swap below is
                // four non-atomic renames, and a partial one leaves `target`
                // absent — every later iteration would then fail at its first
                // rename, freezing `swaps` and silently ending the race.
                if fs::symlink_metadata(&target).is_err() {
                    let _ = fs::rename(&staging, &target);
                } else if fs::symlink_metadata(&decoy).is_err() {
                    let _ = fs::rename(&target, &decoy);
                    let _ = fs::rename(&staging, &target);
                }

                // Swap the symlink in for the real file, then back.
                if fs::rename(&target, &staging).is_ok()
                    && fs::rename(&decoy, &target).is_ok()
                    && fs::rename(&target, &decoy).is_ok()
                    && fs::rename(&staging, &target).is_ok()
                {
                    swaps.fetch_add(1, Ordering::Relaxed);
                }
            }
        })
    };

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut leaks = 0_u32;
    let mut reads = 0_u32;
    while Instant::now() < deadline {
        if let Ok(MontyObject::String(s)) = read_text(&mut mounts, "/mnt/data/file.txt") {
            reads += 1;
            if s.contains(SECRET) {
                leaks += 1;
            }
        }
    }
    stop.store(true, Ordering::Relaxed);
    swapper.join().unwrap();

    // Guard against the test silently doing nothing: both sides must have run.
    assert!(reads > 0, "no read ever succeeded — the scenario did not run");
    assert!(
        swaps.load(Ordering::Relaxed) > 0,
        "no swap ever completed — the scenario did not run"
    );
    assert_eq!(leaks, 0, "HOST FILE DISCLOSURE: a racing read returned host content");
}

/// Writes must be confined too: a rejected write must not create or modify
/// anything outside the mount.
#[test]
fn write_through_swapped_directory_cannot_escape() {
    if !symlinks_supported() {
        return;
    }
    let mount_dir = TempDir::new().unwrap();
    let outside_dir = TempDir::new().unwrap();
    let outside_file = outside_dir.path().join("target.txt");
    fs::write(&outside_file, SECRET).unwrap();

    let data = mount_dir.path().join("data");
    fs::create_dir(&data).unwrap();

    let mut mounts = mount_rw(mount_dir.path());

    fs::remove_dir_all(&data).unwrap();
    symlink_dir(outside_dir.path(), &data);

    let _ = dispatch(
        &mut mounts,
        OsFunctionCall::WriteText(monty_types::PathStringDataArgs {
            path: "/mnt/data/target.txt".into(),
            data: "clobbered".to_owned(),
        }),
    );
    assert_eq!(
        fs::read_to_string(&outside_file).unwrap(),
        SECRET,
        "the out-of-mount file was modified"
    );

    // ...and no new file was created out there either.
    let created: Vec<PathBuf> = fs::read_dir(outside_dir.path())
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p != &outside_file)
        .collect();
    assert!(created.is_empty(), "files were created outside the mount: {created:?}");
}

/// Relative symlink targets are followed normally; paired with the test below,
/// this pins the rule that decides which links work.
#[test]
fn relative_symlink_target_is_followed_inside_the_mount() {
    if !symlinks_supported() {
        return;
    }
    let mount_dir = TempDir::new().unwrap();
    fs::write(mount_dir.path().join("hello.txt"), "in-mount").unwrap();
    symlink_file("hello.txt", mount_dir.path().join("link.txt"));

    let mut mounts = mount_rw(mount_dir.path());

    assert_eq!(
        read_text(&mut mounts, "/mnt/link.txt").unwrap(),
        MontyObject::String("in-mount".to_owned())
    );
}

/// An absolute symlink target is refused even pointing back into the same mount.
///
/// A descriptor has no path, so a leading `/` cannot be interpreted and
/// "absolute but inside" is indistinguishable from "absolute and outside".
/// Re-rooting it ourselves would restore the check-then-use this design removes.
#[test]
fn absolute_symlink_target_is_refused_even_inside_the_mount() {
    if !symlinks_supported() {
        return;
    }
    let mount_dir = TempDir::new().unwrap();
    fs::write(mount_dir.path().join("hello.txt"), "in-mount").unwrap();
    symlink_file(mount_dir.path().join("hello.txt"), mount_dir.path().join("abs.txt"));

    let mut mounts = mount_rw(mount_dir.path());

    // The target is the same file the relative link reaches successfully.
    assert!(
        matches!(
            read_text(&mut mounts, "/mnt/abs.txt"),
            Err(MountError::PathEscape { .. })
        ),
        "absolute in-mount symlink should be refused"
    );

    // `pathlib` predicates cannot raise, so they answer `False` instead.
    for call in [
        OsFunctionCall::Exists("/mnt/abs.txt".into()),
        OsFunctionCall::IsFile("/mnt/abs.txt".into()),
        OsFunctionCall::IsDir("/mnt/abs.txt".into()),
    ] {
        assert_eq!(dispatch(&mut mounts, call).unwrap(), MontyObject::Bool(false));
    }

    // `is_symlink` does not follow the final component, so it still sees a link.
    assert_eq!(
        dispatch(&mut mounts, OsFunctionCall::IsSymlink("/mnt/abs.txt".into())).unwrap(),
        MontyObject::Bool(true)
    );
}

/// A genuine host permission error must not be reported as a path escape.
///
/// `cap-std` answers both with `PermissionDenied`; only the errno separates them,
/// its escape error being synthetic. Conflating them would make `PathEscape`
/// mean "something was denied" rather than "the boundary caught something".
#[test]
#[cfg(unix)]
fn genuine_permission_error_is_not_reported_as_an_escape() {
    let mount_dir = TempDir::new().unwrap();
    let unreadable = mount_dir.path().join("locked.txt");
    fs::write(&unreadable, "in-mount").unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();

    // Running as root defeats the mode bits, leaving nothing to assert.
    if fs::read(&unreadable).is_ok() {
        eprintln!("skipped: running as root, mode 0o000 is not enforced");
        return;
    }

    let mut mounts = mount_rw(mount_dir.path());
    let outcome = read_text(&mut mounts, "/mnt/locked.txt");

    assert!(
        matches!(&outcome, Err(MountError::Io(err, _)) if err.kind() == ErrorKind::PermissionDenied),
        "expected a plain IO permission error, got {outcome:?}"
    );
}

/// Overlay mode must classify an in-mount permission failure the way a direct
/// mount does, instead of collapsing every lookup failure into "missing".
///
/// Collapsing turned a denied `open` into `FileNotFoundError`, let `append`
/// silently succeed into the overlay — shadowing a real file it could not see —
/// and made `stat` return a result for an unreadable path.
#[test]
#[cfg(unix)]
fn overlay_permission_errors_match_direct_mode() {
    let probe = TempDir::new().unwrap();
    let sub = probe.path().join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("f.txt"), "content").unwrap();
    fs::set_permissions(&sub, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(sub.join("f.txt")).is_ok() {
        eprintln!("skipped: running as root, mode 0o000 is not enforced");
        return;
    }
    drop(probe);

    for mode in [MountMode::ReadWrite, MountMode::OverlayMemory(OverlayState::new())] {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("f.txt"), "content").unwrap();
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o000)).unwrap();

        let mut mounts = MountTable::new();
        mounts.mount("/mnt", dir.path(), mode, None).expect("failed to mount");

        for call in [
            OsFunctionCall::ReadText("/mnt/sub/f.txt".into()),
            OsFunctionCall::Open(OpenCallArgs {
                path: "/mnt/sub/f.txt".into(),
                mode: FileMode::Read(false),
            }),
            OsFunctionCall::AppendText(PathStringDataArgs {
                path: "/mnt/sub/f.txt".into(),
                data: "x".to_owned(),
            }),
            OsFunctionCall::Stat("/mnt/sub/f.txt".into()),
        ] {
            let label = format!("{call:?}");
            let outcome = dispatch(&mut mounts, call);
            assert!(
                matches!(&outcome, Err(MountError::Io(err, _)) if err.kind() == ErrorKind::PermissionDenied),
                "{label}: expected a permission error, got {outcome:?}"
            );
        }

        // Predicates still must not raise.
        for call in [
            OsFunctionCall::Exists("/mnt/sub/f.txt".into()),
            OsFunctionCall::IsFile("/mnt/sub/f.txt".into()),
            OsFunctionCall::IsDir("/mnt/sub/f.txt".into()),
        ] {
            let label = format!("{call:?}");
            assert_eq!(
                dispatch(&mut mounts, call).unwrap(),
                MontyObject::Bool(false),
                "{label}: predicates must answer False, not raise"
            );
        }

        fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// The same guarantee as the test above, on every platform.
///
/// That one needs Unix mode bits; this uses the read-only attribute, which
/// Windows honours even for an elevated process. It is what actually exercises
/// `map_io`'s errno check on Windows, where std's error plumbing differs and a
/// misclassification would report a plain denial as a sandbox-boundary error.
#[test]
fn denied_write_is_not_reported_as_an_escape() {
    let mount_dir = TempDir::new().unwrap();
    let locked = mount_dir.path().join("locked.txt");
    fs::write(&locked, "content").unwrap();
    set_readonly(&locked, true);

    // Root ignores Unix mode bits, leaving nothing to assert.
    if fs::OpenOptions::new().write(true).open(&locked).is_ok() {
        set_readonly(&locked, false);
        eprintln!("skipped: host permits writing a read-only file");
        return;
    }

    let mut mounts = mount_rw(mount_dir.path());
    let outcome = dispatch(
        &mut mounts,
        OsFunctionCall::WriteText(PathStringDataArgs {
            path: "/mnt/locked.txt".into(),
            data: "x".to_owned(),
        }),
    );

    // Restore before asserting: Windows cannot delete a read-only file, so a
    // failure here would otherwise break the temp-dir cleanup too.
    set_readonly(&locked, false);

    assert!(
        matches!(&outcome, Err(MountError::Io(err, _)) if err.kind() == ErrorKind::PermissionDenied),
        "expected a plain IO permission error, got {outcome:?}"
    );
}

/// The open is the first thing mount construction does with the host name.
///
/// Validating a path and then opening it would be a check-to-open race: a host
/// able to rename in the parent could swap a symlink into the gap and hand the
/// mount a descriptor on a directory it never shared. Nothing may observe the
/// name beforehand, so a rejection the open itself accounts for — a file, a
/// missing path — has to arrive as a failure of the open. That is the symptom a
/// reintroduced pre-check would change, since it would answer first.
///
/// Construction can still fail *after* the open, canonicalizing the diagnostic
/// label; that ordering is the point, so those failures read differently.
#[test]
fn mount_construction_rejects_at_the_open_first() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("not-a-directory.txt");
    fs::write(&file, "content").unwrap();

    // A non-directory: rejected by `O_DIRECTORY` (a handle `metadata()` on
    // Windows), not by a `is_dir()` stat on a path resolved separately.
    let mut mounts = MountTable::new();
    match mounts.mount("/mnt", &file, MountMode::ReadWrite, None) {
        Err(MountError::InvalidMount(msg)) => assert!(
            msg.starts_with("cannot open host path"),
            "a pre-check answered before the open: {msg}"
        ),
        outcome => panic!("expected InvalidMount for a file, got {outcome:?}"),
    }

    let mut mounts = MountTable::new();
    match mounts.mount("/mnt", dir.path().join("absent"), MountMode::ReadWrite, None) {
        Err(MountError::InvalidMount(msg)) => assert!(
            msg.starts_with("cannot open host path"),
            "a pre-check answered before the open: {msg}"
        ),
        outcome => panic!("expected InvalidMount for a missing path, got {outcome:?}"),
    }
}

/// Whether a search-only directory can be mounted is platform-specific: Linux
/// opens directories with `O_PATH` and accepts one, macOS and the BSDs need
/// read permission and reject it where the old canonicalize-and-stat did not.
///
/// Documented in `limitations/filesystem.md`; asserted here so that where it
/// is refused, the failure stays a clean `InvalidMount` at construction rather
/// than a surprise on the first read.
#[test]
#[cfg(unix)]
fn search_only_directory_mountability() {
    let base = TempDir::new().unwrap();
    let target = base.path().join("searchonly");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o111)).unwrap();

    let mut mounts = MountTable::new();
    let outcome = mounts.mount("/mnt", &target, MountMode::ReadWrite, None);

    // Restore before asserting so the temp dir can still be cleaned up.
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();

    // Linux's `O_PATH`, or running as root, which ignores mode bits.
    if outcome.is_ok() {
        return;
    }
    assert!(
        matches!(&outcome, Err(MountError::InvalidMount(msg)) if msg.contains("cannot open host path")),
        "expected InvalidMount for a search-only directory, got {outcome:?}"
    );
}

/// The mount root itself cannot be deleted through the mount.
///
/// It has no name inside the mount, so there is nothing to unlink it from:
/// `rmdir` is refused by the explicit guard (like rename), `unlink` fails at
/// the OS, and the host directory survives both.
#[test]
fn deleting_the_mount_root_is_refused() {
    let mount_dir = TempDir::new().unwrap();
    let mut mounts = mount_rw(mount_dir.path());

    let rmdir = dispatch(&mut mounts, OsFunctionCall::Rmdir("/mnt".into()));
    assert!(rmdir.is_err(), "rmdir of the mount root must fail, got {rmdir:?}");
    assert!(mount_dir.path().exists(), "host directory must survive rmdir");

    let unlink = dispatch(&mut mounts, OsFunctionCall::Unlink("/mnt".into()));
    assert!(unlink.is_err(), "unlink of the mount root must fail, got {unlink:?}");
    assert!(mount_dir.path().exists(), "host directory must survive unlink");
}

/// Overlay `rmdir` of the mount root must never touch the host directory.
///
/// The overlay now refuses the call outright (see
/// `rmdir_of_mount_root_is_refused_in_both_modes` in `fs.rs`); this pins the
/// host-side guarantee that survives whatever the in-memory answer is.
#[test]
fn overlay_rmdir_of_the_mount_root_leaves_the_host_directory() {
    let mount_dir = TempDir::new().unwrap();
    let mut mounts = MountTable::new();
    mounts
        .mount(
            "/mnt",
            mount_dir.path(),
            MountMode::OverlayMemory(OverlayState::new()),
            None,
        )
        .expect("failed to mount");

    let rmdir = dispatch(&mut mounts, OsFunctionCall::Rmdir("/mnt".into()));
    assert!(
        rmdir.is_err(),
        "overlay rmdir of the mount root must fail, got {rmdir:?}"
    );
    assert!(mount_dir.path().exists(), "host directory must survive overlay rmdir");
}

/// `mkdir(parents=True)` through an escaping symlink cannot reach the host.
///
/// Both modes refuse it with `PathEscape` (`PermissionError` in the sandbox):
/// direct at the descriptor, overlay via `classify_write_target` — which used
/// to treat the unobservable link as absent and build an in-memory shadow
/// tree. Nothing may appear on the host and the link's real target must stay
/// unreadable.
#[test]
fn mkdir_parents_under_an_escaping_symlink_cannot_reach_the_host() {
    if !symlinks_supported() {
        return;
    }
    let mount_dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("secret.txt"), SECRET).unwrap();
    symlink_dir(outside.path(), mount_dir.path().join("evil"));

    let mkdir_call = || {
        OsFunctionCall::Mkdir(MkdirCallArgs {
            path: "/mnt/evil/foo".into(),
            parents: true,
            exist_ok: false,
        })
    };

    let mut direct = mount_rw(mount_dir.path());
    let outcome = dispatch(&mut direct, mkdir_call());
    assert!(
        matches!(&outcome, Err(MountError::PathEscape { .. })),
        "direct mkdir through an escaping symlink must be refused, got {outcome:?}"
    );
    assert!(
        !outside.path().join("foo").exists(),
        "directory created outside the mount"
    );

    let mut overlay = MountTable::new();
    overlay
        .mount(
            "/mnt",
            mount_dir.path(),
            MountMode::OverlayMemory(OverlayState::new()),
            None,
        )
        .expect("failed to mount");
    let outcome = dispatch(&mut overlay, mkdir_call());
    assert!(
        matches!(&outcome, Err(MountError::PathEscape { .. })),
        "overlay mkdir through an escaping symlink must be refused, got {outcome:?}"
    );
    let outcome = dispatch(
        &mut overlay,
        OsFunctionCall::WriteText(PathStringDataArgs {
            path: "/mnt/evil/foo/x.txt".into(),
            data: "data".to_owned(),
        }),
    );
    assert!(
        matches!(&outcome, Err(MountError::PathEscape { .. })),
        "overlay write under an escaping symlink must be refused, got {outcome:?}"
    );
    assert!(
        !outside.path().join("foo").exists(),
        "directory created outside the mount"
    );
    assert!(!outside.path().join("x.txt").exists(), "file created outside the mount");

    let outcome = read_text(&mut overlay, "/mnt/evil/secret.txt");
    let leaked = matches!(&outcome, Ok(MontyObject::String(s)) if s.contains(SECRET));
    assert!(!leaked, "HOST FILE DISCLOSURE: read followed the escaping symlink");
    assert!(outcome.is_err(), "outside read must be refused, got {outcome:?}");
}

/// Toggles the read-only attribute (Windows) / write mode bits (Unix).
fn set_readonly(path: &Path, readonly: bool) {
    let mut perms = fs::metadata(path).expect("failed to stat").permissions();
    perms.set_readonly(readonly);
    fs::set_permissions(path, perms).expect("failed to set permissions");
}

/// A FIFO in the mount is refused, and refused *promptly*.
///
/// Reading one with no writer blocks forever, and mount I/O runs on the host
/// thread servicing the sandbox, so a regression here hangs the host rather
/// than failing. The dispatch runs on its own thread so that shows up as a
/// failed assertion instead of a wedged test binary.
#[test]
#[cfg(unix)]
fn fifo_in_the_mount_is_refused_promptly() {
    let mount_dir = TempDir::new().unwrap();
    make_fifo(&mount_dir.path().join("pipe"));

    let (tx, rx) = mpsc::channel();
    let host = mount_dir.path().to_path_buf();
    thread::spawn(move || {
        let mut mounts = mount_rw(&host);
        tx.send(format!("{:?}", read_text(&mut mounts, "/mnt/pipe"))).ok();
    });

    let outcome = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("reading a FIFO blocked the servicing thread");
    assert!(
        outcome.contains("PermissionDenied"),
        "expected a refusal, got {outcome}"
    );
}

/// The guard holds when the FIFO arrives *after* the path check.
///
/// This is the race the pre-open check cannot win: a second session renames a
/// host-planted FIFO over a regular file between the check and the open. The
/// open is non-blocking and the handle is re-checked, so neither the hang nor a
/// successful FIFO read is reachable. Verified red with either half removed.
/// A soak like the one above, so it is gated the same way.
#[test]
#[cfg(unix)]
fn fifo_raced_into_place_between_check_and_open_is_refused() {
    if !soak_enabled() {
        return;
    }
    let mount_dir = TempDir::new().unwrap();
    let target = mount_dir.path().join("file.txt");
    let regular = mount_dir.path().join("regular");
    let fifo = mount_dir.path().join("fifo");
    fs::write(&target, "public").unwrap();
    fs::write(&regular, "public").unwrap();
    make_fifo(&fifo);

    let stop = Arc::new(AtomicBool::new(false));
    let swapper = {
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                // Restore a known-good state first so one failed rename cannot
                // wedge the loop and silently stop the race.
                if !target.exists() {
                    let _ = fs::rename(&regular, &target).or_else(|_| fs::rename(&fifo, &target));
                }
                let _ = fs::rename(&target, &regular).and_then(|()| fs::rename(&fifo, &target));
                let _ = fs::rename(&target, &fifo).and_then(|()| fs::rename(&regular, &target));
            }
        })
    };

    let (tx, rx) = mpsc::channel();
    let host = mount_dir.path().to_path_buf();
    let reader = thread::spawn(move || {
        let mut mounts = mount_rw(&host);
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            match read_text(&mut mounts, "/mnt/file.txt") {
                // The FIFO has no writer, so any success is a regular file.
                Ok(MontyObject::String(s)) => assert_eq!(s, "public"),
                Ok(other) => panic!("unexpected value: {other:?}"),
                Err(_) => {} // Missing mid-swap, or refused as a FIFO: both fine.
            }
        }
        tx.send(()).ok();
    });

    let finished = rx.recv_timeout(Duration::from_secs(30)).is_ok();
    stop.store(true, Ordering::Relaxed);
    swapper.join().unwrap();
    // Assert before joining the reader: a read that blocked on the FIFO never
    // returns, so joining first would hang the test binary rather than fail it
    // — exactly the regression this test exists to catch.
    assert!(finished, "a read blocked on the raced-in FIFO");
    reader.join().unwrap();
}

/// Creates a FIFO, the one special file a test can make portably across Unix.
#[cfg(unix)]
fn make_fifo(path: &Path) {
    mkfifo(path, Mode::from_bits_truncate(0o644)).expect("failed to create FIFO");
}
