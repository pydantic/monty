//! Directory-swap and concurrent-rename coverage for the mount boundary.
//!
//! These do **not** cover the identity check — all three pass with it removed,
//! because a swap staged from outside the crate lands before resolution, where
//! the boundary check already rejects it. Its semantics live in the unit tests
//! in `src/file_identity.rs`.

#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_dir as win_symlink_dir;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use monty_fs::{MountCallOutcome, MountError, MountMode, MountTable};
use monty_types::{MontyObject, OsFunctionCall, PathStringDataArgs};
use tempfile::TempDir;

/// Cross-platform symlink to a directory. Windows needs Developer Mode or
/// elevation.
///
/// Duplicated from `fs_security.rs` to stay off the paths #656 moves into
/// `tests/common/`; fold it in once that lands.
fn symlink_dir(original: impl AsRef<Path>, link: impl AsRef<Path>) {
    #[cfg(unix)]
    symlink(original.as_ref(), link.as_ref()).expect("failed to create directory symlink");
    #[cfg(windows)]
    win_symlink_dir(original.as_ref(), link.as_ref())
        .expect("failed to create directory symlink (enable Windows Developer Mode or run elevated)");
}

/// Marker written to the out-of-mount file; its appearance in any result is the
/// disclosure these tests guard against.
const SECRET: &str = "SECRET-HOST-CONTENT";

/// Dispatches a call, panicking if the mount table declines to handle it.
fn dispatch(mounts: &mut MountTable, call: OsFunctionCall) -> Result<MontyObject, MountError> {
    match mounts.handle_os_call(call) {
        MountCallOutcome::Handled(result) => result,
        MountCallOutcome::NotHandled(call) => panic!("mount table returned NotHandled: {call:?}"),
    }
}

/// A `ReadWrite` mount whose `/mnt/data` directory can be swapped for one that
/// escapes the mount, standing in for a concurrent session's `os.rename`.
///
/// The swap is between two *ordinary directories*, only a child of the second
/// being a symlink out — the shape that defeats any rename-time symlink
/// filter.
struct SwappableMount {
    mounts: MountTable,
    /// `/mnt/data` in host terms — the directory a read resolves through.
    data_dir: PathBuf,
    /// A sibling directory holding an outbound symlink named `file.txt`.
    escaping_dir: PathBuf,
    _mount_dir: TempDir,
    _outside_dir: TempDir,
}

impl SwappableMount {
    fn new() -> Self {
        let mount_dir = TempDir::new().unwrap();
        let outside_dir = TempDir::new().unwrap();

        let secret = outside_dir.path().join("secret.txt");
        fs::write(&secret, SECRET).unwrap();

        // The path a read targets: /mnt/data/file.txt, entirely in bounds.
        let data_dir = mount_dir.path().join("data");
        fs::create_dir(&data_dir).unwrap();
        fs::write(data_dir.join("file.txt"), "public").unwrap();

        // The pre-existing outbound symlink the sandbox can relocate but never
        // create. Pointing it at the *directory* means the whole subtree leaks.
        let escaping_dir = mount_dir.path().join("escape");
        fs::create_dir(&escaping_dir).unwrap();
        symlink_dir(outside_dir.path(), escaping_dir.join("file.txt"));

        let mut mounts = MountTable::new();
        mounts
            .mount("/mnt", mount_dir.path(), MountMode::ReadWrite, None)
            .expect("failed to configure mount");

        Self {
            mounts,
            data_dir,
            escaping_dir,
            _mount_dir: mount_dir,
            _outside_dir: outside_dir,
        }
    }

    /// Swaps `data` and `escape`, so `/mnt/data/file.txt` now traverses the
    /// outbound symlink. This is what a concurrent `os.rename` achieves.
    fn swap_directories(&self) {
        let staging = self.data_dir.with_file_name("staging");
        fs::rename(&self.data_dir, &staging).unwrap();
        fs::rename(&self.escaping_dir, &self.data_dir).unwrap();
        fs::rename(&staging, &self.escaping_dir).unwrap();
    }
}

/// Swapping two ordinary directories, where only a child of one is an outbound
/// symlink, must not open a read path out of the mount.
#[test]
fn read_rejects_path_through_swapped_directory() {
    let mut scenario = SwappableMount::new();

    // Before the swap the read is ordinary and in bounds.
    let before = dispatch(
        &mut scenario.mounts,
        OsFunctionCall::ReadText("/mnt/data/file.txt".into()),
    );
    assert!(
        matches!(&before, Ok(MontyObject::String(s)) if s == "public"),
        "expected the pre-swap read to succeed, got {before:?}"
    );

    scenario.swap_directories();

    // `/mnt/data/file.txt` now traverses a symlink out of the mount, so it must
    // be refused — by the boundary check here, since resolution runs afresh.
    let after = dispatch(
        &mut scenario.mounts,
        OsFunctionCall::ReadText("/mnt/data/secret.txt".into()),
    );
    let leaked = matches!(&after, Ok(MontyObject::String(s)) if s.contains(SECRET));
    assert!(!leaked, "HOST FILE DISCLOSURE: read returned out-of-mount content");
}

/// A rejected write must leave the out-of-mount file untouched — `fs::write`
/// would have truncated it while opening, before the rejection.
#[test]
fn rejected_write_does_not_truncate_out_of_mount_file() {
    let mount_dir = TempDir::new().unwrap();
    let outside_dir = TempDir::new().unwrap();

    let secret = outside_dir.path().join("secret.txt");
    fs::write(&secret, SECRET).unwrap();

    let data_dir = mount_dir.path().join("data");
    fs::create_dir(&data_dir).unwrap();
    fs::write(data_dir.join("file.txt"), "public").unwrap();

    let mut mounts = MountTable::new();
    mounts
        .mount("/mnt", mount_dir.path(), MountMode::ReadWrite, None)
        .unwrap();

    // Point the whole `data` directory out of the mount.
    fs::remove_dir_all(&data_dir).unwrap();
    symlink_dir(outside_dir.path(), &data_dir);

    let outcome = dispatch(
        &mut mounts,
        OsFunctionCall::WriteText(PathStringDataArgs {
            path: "/mnt/data/secret.txt".into(),
            data: "clobbered".to_owned(),
        }),
    );
    assert!(
        matches!(outcome, Err(MountError::PathEscape { .. })),
        "expected PathEscape, got {outcome:?}"
    );
    assert_eq!(
        fs::read_to_string(&secret).unwrap(),
        SECRET,
        "the out-of-mount file was modified"
    );
}

/// The reported vector end to end: two sessions on one shared `ReadWrite`
/// mount, one reading while the other renames.
///
/// Asserts only that no read discloses out-of-mount content, never that the
/// race is won — win rates swing wildly across filesystems, so requiring a win
/// would be flaky exactly where substitution is hardest to land.
#[test]
fn concurrent_rename_never_discloses_out_of_mount_content() {
    let scenario = SwappableMount::new();
    let SwappableMount {
        mut mounts,
        data_dir,
        escaping_dir,
        _mount_dir,
        _outside_dir,
    } = scenario;

    let stop = Arc::new(AtomicBool::new(false));
    let swapper = {
        let stop = Arc::clone(&stop);
        let staging = data_dir.with_file_name("staging");
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                // Ignore failures: the reader may hold the directory busy, and
                // a partially-applied swap is exactly the state under test.
                let _ = fs::rename(&data_dir, &staging);
                let _ = fs::rename(&escaping_dir, &data_dir);
                let _ = fs::rename(&staging, &escaping_dir);
            }
        })
    };

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut leaks = 0_u32;
    while Instant::now() < deadline {
        for path in ["/mnt/data/file.txt", "/mnt/data/secret.txt"] {
            if let MountCallOutcome::Handled(Ok(MontyObject::String(s))) =
                mounts.handle_os_call(OsFunctionCall::ReadText(path.into()))
                && s.contains(SECRET)
            {
                leaks += 1;
            }
        }
    }
    stop.store(true, Ordering::Relaxed);
    swapper.join().unwrap();

    assert_eq!(leaks, 0, "HOST FILE DISCLOSURE: a racing read returned host content");
}
