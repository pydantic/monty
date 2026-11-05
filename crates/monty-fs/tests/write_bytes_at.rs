use std::fs;

use monty_fs::{Mount, MountCallOutcome, MountMode, MountTable, OverlayState};
use monty_types::{ExcType, MontyObject, OsFunctionCall, WriteBytesAtArgs};
use tempfile::TempDir;

#[expect(dead_code, reason = "shared helpers; these tests only create file symlinks")]
mod common;

fn write(offset: u64, data: &[u8]) -> OsFunctionCall {
    OsFunctionCall::WriteBytesAt(WriteBytesAtArgs {
        path: "/mnt/file".into(),
        offset,
        data: data.to_vec(),
    })
}

fn dispatch(table: &mut MountTable, call: OsFunctionCall) -> Result<MontyObject, monty_fs::MountError> {
    let MountCallOutcome::Handled(result) = table.handle_os_call(call) else {
        panic!("unmounted test path")
    };
    result
}

fn table(dir: &TempDir, overlay: bool, limit: Option<u64>) -> MountTable {
    let mode = if overlay {
        MountMode::OverlayMemory(OverlayState::new())
    } else {
        MountMode::ReadWrite
    };
    let mount = Mount::new("/mnt", dir.path(), mode, limit).unwrap();
    let mut table = MountTable::new();
    table.push_mount(mount).unwrap();
    table
}

#[test]
fn overwrites_preserve_suffixes_and_gaps_without_writing_through_overlays() {
    for overlay in [false, true] {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("file"), b"abcdef").unwrap();
        let mut table = table(&dir, overlay, None);
        assert_eq!(dispatch(&mut table, write(2, b"XY")).unwrap(), MontyObject::int(2));
        assert_eq!(dispatch(&mut table, write(8, b"Z")).unwrap(), MontyObject::int(1));
        dispatch(&mut table, write(100, b"")).unwrap();
        assert_eq!(
            dispatch(&mut table, OsFunctionCall::ReadBytes("/mnt/file".into())).unwrap(),
            MontyObject::bytes(b"abXYef\0\0Z".to_vec())
        );
        if overlay {
            assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"abcdef");
        }
    }
}

#[test]
fn gaps_and_materialization_are_charged_before_mutation() {
    for overlay in [false, true] {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("file"), b"abc").unwrap();
        let mut table = table(&dir, overlay, Some(4));
        assert!(dispatch(&mut table, write(10, b"x")).is_err());
        assert_eq!(
            dispatch(&mut table, OsFunctionCall::ReadBytes("/mnt/file".into())).unwrap(),
            MontyObject::bytes(b"abc".to_vec())
        );
        dispatch(&mut table, write(6, b"")).unwrap();
        // Native: one payload byte + three gap bytes. Overlay: seven materialized bytes.
        assert_eq!(dispatch(&mut table, write(6, b"x")).is_err(), overlay);
    }
}

#[test]
fn invalid_ranges_missing_files_and_read_only_mounts_are_rejected() {
    for overlay in [false, true] {
        let dir = TempDir::new().unwrap();
        let mut table = table(&dir, overlay, None);
        let error = dispatch(&mut table, write(0, b"x")).unwrap_err();
        assert_eq!(error.into_exception().exc_type(), ExcType::FileNotFoundError);
        fs::write(dir.path().join("file"), b"abc").unwrap();
        assert!(dispatch(&mut table, write(i64::MAX as u64, b"x")).is_err());
        assert_eq!(fs::read(dir.path().join("file")).unwrap(), b"abc");
    }
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("file"), b"abc").unwrap();
    let mut table = MountTable::new();
    table.mount("/mnt", dir.path(), MountMode::ReadOnly, None).unwrap();
    assert!(dispatch(&mut table, write(0, b"x")).is_err());
}

#[test]
fn escaping_symlinks_cannot_be_overwritten() {
    if !common::symlinks_supported() {
        return;
    }
    for overlay in [false, true] {
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret"), b"unchanged").unwrap();
        common::symlink_file(outside.path().join("secret"), dir.path().join("file"));
        let mut table = table(&dir, overlay, None);
        assert!(dispatch(&mut table, write(0, b"changed")).is_err());
        assert_eq!(fs::read(outside.path().join("secret")).unwrap(), b"unchanged");
    }
}

#[test]
fn nested_overlay_routes_writes_without_touching_either_backing_directory() {
    let parent = TempDir::new().unwrap();
    let child = TempDir::new().unwrap();
    fs::create_dir(parent.path().join("nested")).unwrap();
    fs::write(parent.path().join("nested/file"), b"parent").unwrap();
    fs::write(child.path().join("file"), b"child").unwrap();
    let mut table = table(&parent, false, None);
    table
        .mount(
            "/mnt/nested",
            child.path(),
            MountMode::OverlayMemory(OverlayState::new()),
            None,
        )
        .unwrap();
    let call = OsFunctionCall::WriteBytesAt(WriteBytesAtArgs {
        path: "/mnt/nested/file".into(),
        offset: 1,
        data: b"X".to_vec(),
    });
    dispatch(&mut table, call).unwrap();
    assert_eq!(
        dispatch(&mut table, OsFunctionCall::ReadBytes("/mnt/nested/file".into())).unwrap(),
        MontyObject::bytes(b"cXild".to_vec())
    );
    assert_eq!(fs::read(parent.path().join("nested/file")).unwrap(), b"parent");
    assert_eq!(fs::read(child.path().join("file")).unwrap(), b"child");
}
