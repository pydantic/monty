//! Exclusive creation must preserve occupied names and never write through an overlay.

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    fs,
    path::Path,
    sync::{Arc, Barrier},
    thread,
};

use monty_fs::{Mount, MountCallOutcome, MountError, MountMode, MountTable, OverlayState};
use monty_types::{
    CreateExclusiveCallArgs, ExcType, FileMode, MontyFileHandle, MontyObject, OsFunctionCall, PathBytesDataArgs,
};
use tempfile::TempDir;

#[expect(dead_code, reason = "shared helpers; this test only needs symlink creation")]
mod common;
use common::{symlink_dir, symlink_file, symlinks_supported};

/// Open a fresh table over the same backing directory.
fn mount(dir: &TempDir, mode: MountMode) -> MountTable {
    let mut table = MountTable::new();
    table.mount("/mnt", dir.path(), mode, None).unwrap();
    table
}

/// Exercise the public typed request directly, before a Python API emits it.
fn create(path: &str, mode: &str) -> OsFunctionCall {
    OsFunctionCall::CreateExclusive(CreateExclusiveCallArgs {
        path: path.into(),
        mode: mode.parse().unwrap(),
        permissions: 0o600,
    })
}

/// Require the mount to handle the request.
fn dispatch(table: &mut MountTable, call: OsFunctionCall) -> Result<MontyObject, MountError> {
    let MountCallOutcome::Handled(result) = table.handle_os_call(call) else {
        panic!("test path must be covered");
    };
    result
}

/// Exercise both writable backends.
fn writable_modes() -> [MountMode; 2] {
    [MountMode::ReadWrite, MountMode::OverlayMemory(OverlayState::new())]
}

#[test]
fn creates_empty_file_and_returns_requested_handle() {
    for mode in ["r", "rb", "w", "wb", "a", "ab", "w+b"] {
        for backend in writable_modes() {
            let dir = TempDir::new().unwrap();
            let mut table = mount(&dir, backend);
            let result = dispatch(&mut table, create("/mnt/file", mode)).unwrap();
            assert_eq!(
                result,
                MontyObject::file_handle(MontyFileHandle {
                    path: "/mnt/file".to_owned(),
                    mode: mode.parse().unwrap(),
                    position: 0,
                })
            );
            assert_eq!(
                dispatch(&mut table, OsFunctionCall::ReadBytes("/mnt/file".into())).unwrap(),
                MontyObject::bytes(vec![])
            );
            assert_eq!(
                dispatch(&mut table, create("/mnt/file", mode))
                    .unwrap_err()
                    .into_exception()
                    .exc_type(),
                ExcType::FileExistsError
            );
        }
    }
}

#[test]
fn collisions_preserve_files_and_directories() {
    for backend in writable_modes() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("file"), b"sentinel").unwrap();
        fs::create_dir(dir.path().join("directory")).unwrap();
        let mut table = mount(&dir, backend);
        for name in ["file", "directory"] {
            let error = dispatch(&mut table, create(&format!("/mnt/{name}"), "a"))
                .unwrap_err()
                .into_exception();
            assert_eq!(error.exc_type(), ExcType::FileExistsError);
        }
        assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"sentinel");
        assert!(dir.path().join("directory").is_dir());
    }
}

#[test]
fn existing_and_dangling_symlinks_are_collisions() {
    if !symlinks_supported() {
        return;
    }
    for backend in writable_modes() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("target"), b"sentinel").unwrap();
        symlink_file("target", dir.path().join("link"));
        symlink_file("absent", dir.path().join("dangling"));
        let mut table = mount(&dir, backend);
        for name in ["link", "dangling"] {
            let error = dispatch(&mut table, create(&format!("/mnt/{name}"), "w"))
                .unwrap_err()
                .into_exception();
            assert_eq!(error.exc_type(), ExcType::FileExistsError);
        }
        assert_eq!(fs::read(dir.path().join("target")).unwrap(), b"sentinel");
        assert!(!dir.path().join("absent").exists());
        for (name, target) in [("link", "target"), ("dangling", "absent")] {
            let path = dir.path().join(name);
            assert!(fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
            assert_eq!(fs::read_link(path).unwrap(), Path::new(target));
        }
    }
}

#[test]
fn creation_is_a_write_even_for_a_read_handle() {
    let dir = TempDir::new().unwrap();
    let mut table = mount(&dir, MountMode::ReadOnly);
    let error = dispatch(&mut table, create("/mnt/file", "r"))
        .unwrap_err()
        .into_exception();
    assert_eq!(error.exc_type(), ExcType::PermissionError);
    assert!(!dir.path().join("file").exists());
}

#[test]
fn missing_parents_are_not_created() {
    for backend in writable_modes() {
        let dir = TempDir::new().unwrap();
        let mut table = mount(&dir, backend);
        let error = dispatch(&mut table, create("/mnt/missing/file", "w"))
            .unwrap_err()
            .into_exception();
        assert_eq!(error.exc_type(), ExcType::FileNotFoundError);
        assert!(!dir.path().join("missing").exists());
    }
}

#[test]
fn overlay_creation_and_writes_never_reach_backing_storage() {
    let dir = TempDir::new().unwrap();
    {
        let mut table = mount(&dir, MountMode::OverlayMemory(OverlayState::new()));
        dispatch(&mut table, create("/mnt/file", "wb")).unwrap();
        dispatch(
            &mut table,
            OsFunctionCall::WriteBytes(PathBytesDataArgs {
                path: "/mnt/file".into(),
                data: b"overlay bytes".to_vec(),
            }),
        )
        .unwrap();
        assert_eq!(
            dispatch(&mut table, OsFunctionCall::ReadBytes("/mnt/file".into())).unwrap(),
            MontyObject::bytes(b"overlay bytes".to_vec())
        );
        assert!(!dir.path().join("file").exists());
    }
    let mut next_feed = mount(&dir, MountMode::OverlayMemory(OverlayState::new()));
    assert_eq!(
        dispatch(&mut next_feed, OsFunctionCall::Exists("/mnt/file".into())).unwrap(),
        MontyObject::bool(false)
    );
    assert!(!dir.path().join("file").exists());
}

#[test]
fn overlay_can_recreate_a_tombstoned_file_without_changing_the_host_file() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("file"), b"original").unwrap();
    let mut table = mount(&dir, MountMode::OverlayMemory(OverlayState::new()));
    dispatch(&mut table, OsFunctionCall::Unlink("/mnt/file".into())).unwrap();
    dispatch(&mut table, create("/mnt/file", "wb")).unwrap();
    dispatch(
        &mut table,
        OsFunctionCall::WriteBytes(PathBytesDataArgs {
            path: "/mnt/file".into(),
            data: b"replacement".to_vec(),
        }),
    )
    .unwrap();
    assert_eq!(
        dispatch(&mut table, OsFunctionCall::ReadBytes("/mnt/file".into())).unwrap(),
        MontyObject::bytes(b"replacement".to_vec())
    );
    assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"original");
    drop(table);
    assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"original");
}

#[test]
fn overlay_memory_limit_rejects_before_inserting() {
    let dir = TempDir::new().unwrap();
    let overlay = Mount::new("/mnt", dir.path(), MountMode::OverlayMemory(OverlayState::new()), None)
        .unwrap()
        .with_memory_usage_limit(0);
    let mut table = MountTable::new();
    table.push_mount(overlay).unwrap();
    let error = dispatch(&mut table, create("/mnt/file", "w"))
        .unwrap_err()
        .into_exception();
    assert_eq!(error.exc_type(), ExcType::MemoryError);
    assert_eq!(
        dispatch(&mut table, OsFunctionCall::Exists("/mnt/file".into())).unwrap(),
        MontyObject::bool(false)
    );
    assert!(!dir.path().join("file").exists());
}

#[test]
fn malformed_arguments_do_not_create_files() {
    for backend in writable_modes() {
        let dir = TempDir::new().unwrap();
        let mut table = mount(&dir, backend);
        for (mode, permissions) in [
            (FileMode::WriteUpdate(false), 0o600),
            (FileMode::Write(false), 0o1000),
            (FileMode::Write(false), 0o001),
            (FileMode::Write(false), 0o010),
            (FileMode::Write(false), 0o100),
            (FileMode::Write(false), 0o777),
        ] {
            let error = dispatch(
                &mut table,
                OsFunctionCall::CreateExclusive(CreateExclusiveCallArgs {
                    path: "/mnt/file".into(),
                    mode,
                    permissions,
                }),
            )
            .unwrap_err()
            .into_exception();
            assert_eq!(error.exc_type(), ExcType::ValueError);
        }
        assert_eq!(
            dispatch(&mut table, OsFunctionCall::Exists("/mnt/file".into())).unwrap(),
            MontyObject::bool(false)
        );
        assert!(!dir.path().join("file").exists());
    }
}

#[cfg(unix)]
#[test]
fn permissions_are_applied_at_creation() {
    for permissions in [0o000, 0o400, 0o600, 0o640] {
        let dir = TempDir::new().unwrap();
        let mut table = mount(&dir, MountMode::ReadWrite);
        let mut call = create("/mnt/file", "w");
        let OsFunctionCall::CreateExclusive(args) = &mut call else {
            unreachable!();
        };
        args.permissions = permissions;
        dispatch(&mut table, call).unwrap();
        // A control file applies the same process umask without changing that global setting.
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(permissions)
            .open(dir.path().join("control"))
            .unwrap();
        let actual = fs::metadata(dir.path().join("file")).unwrap().permissions().mode() & 0o777;
        let expected = fs::metadata(dir.path().join("control")).unwrap().permissions().mode() & 0o777;
        assert_eq!(actual, expected, "requested {permissions:o}");
        assert_eq!(actual & !permissions, 0);
    }
}

#[test]
fn concurrent_native_creators_have_one_winner() {
    let dir = TempDir::new().unwrap();
    let barrier = Arc::new(Barrier::new(8));
    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let mut table = mount(&dir, MountMode::ReadWrite);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                match dispatch(&mut table, create("/mnt/file", "w")) {
                    Ok(_) => 1,
                    Err(error) => {
                        assert_eq!(error.into_exception().exc_type(), ExcType::FileExistsError);
                        0
                    }
                }
            })
        })
        .collect();
    assert_eq!(tasks.into_iter().map(|task| task.join().unwrap()).sum::<usize>(), 1);
}

#[test]
fn traversal_and_outbound_symlinks_cannot_create_outside_mount() {
    let dir = TempDir::new().unwrap();
    let inside = dir.path().join("inside");
    fs::create_dir(&inside).unwrap();
    if symlinks_supported() {
        symlink_dir(dir.path(), inside.join("escape"));
    }
    for backend in writable_modes() {
        let mut table = MountTable::new();
        table.mount("/mnt", &inside, backend, None).unwrap();
        assert!(matches!(
            table.handle_os_call(create("/mnt/../outside", "w")),
            MountCallOutcome::NotHandled(_)
        ));
        if symlinks_supported() {
            assert!(dispatch(&mut table, create("/mnt/escape/outside", "w")).is_err());
        }
    }
    assert!(!dir.path().join("outside").exists());
}
