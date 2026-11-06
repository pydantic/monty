use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
};

use monty_fs::{Mount, MountCallOutcome, MountMode, MountTable, OverlayState};
use monty_types::{ExcType, MontyObject, OsFunctionCall, WriteBytesAtArgs};
use proptest::{prelude::*, test_runner::FileFailurePersistence};
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

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128,
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource("proptest-regressions"))),
        ..ProptestConfig::default()
    })]

    #[test]
    fn positional_write_sequences_match_a_reference_file(
        initial in prop::collection::vec(any::<u8>(), 0..129),
        writes in prop::collection::vec(write_operation(), 1..33),
    ) {
        for overlay in [false, true] {
            let dir = TempDir::new()?;
            fs::write(dir.path().join("file"), &initial)?;
            let mut table = table(&dir, overlay, None);
            let mut reference = tempfile::tempfile()?;
            reference.write_all(&initial)?;

            for (index, (position, data)) in writes.iter().enumerate() {
                let length = reference.metadata()?.len();
                let offset = match position {
                    WriteOffset::Absolute(offset) => u64::from(*offset),
                    WriteOffset::BeforeEnd => length.saturating_sub(1),
                    WriteOffset::End => length,
                    WriteOffset::BeyondEnd(gap) => length + u64::from(*gap),
                };
                reference.seek(SeekFrom::Start(offset))?;
                reference.write_all(data)?;
                let count = dispatch(&mut table, write(offset, data))?;
                prop_assert_eq!(count, MontyObject::int(i64::try_from(data.len())?),
                    "overlay={} step={} offset={}", overlay, index, offset);

                reference.rewind()?;
                let mut expected = Vec::new();
                reference.read_to_end(&mut expected)?;
                let content = dispatch(&mut table, OsFunctionCall::ReadBytes("/mnt/file".into()))?;
                prop_assert_eq!(content, MontyObject::bytes(expected),
                    "overlay={} step={} offset={}", overlay, index, offset);
                let stat = dispatch(&mut table, OsFunctionCall::Stat("/mnt/file".into()))?;
                let fields = stat.as_ref().items().expect("stat returns a namedtuple");
                prop_assert_eq!(fields[6].as_int(), Some(i64::try_from(reference.metadata()?.len())?),
                    "overlay={} step={}", overlay, index);

                if overlay {
                    prop_assert_eq!(fs::read(dir.path().join("file"))?, initial.as_slice(),
                        "overlay wrote through to backing file at step={}", index);
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
enum WriteOffset {
    Absolute(u16),
    BeforeEnd,
    End,
    BeyondEnd(u16),
}

fn write_operation() -> impl Strategy<Value = (WriteOffset, Vec<u8>)> {
    let offset = prop_oneof![
        Just(WriteOffset::Absolute(0)),
        (0_u16..257).prop_map(WriteOffset::Absolute),
        Just(WriteOffset::BeforeEnd),
        Just(WriteOffset::End),
        (1_u16..65).prop_map(WriteOffset::BeyondEnd),
    ];
    let data = prop_oneof![
        1 => Just(Vec::new()),
        3 => prop::collection::vec(any::<u8>(), 1..65),
    ];
    (offset, data)
}
