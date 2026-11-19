use super::*;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
struct Table(#[serde(with = "super")] Vec<LocationEntry>);

#[test]
fn empty_table_round_trips() {
    let bytes = minicbor_serde::to_vec(Table(Vec::new())).unwrap();
    assert_eq!(bytes, [0x41, 1]);
    let restored: Table = minicbor_serde::from_slice(&bytes).unwrap();
    assert!(restored.0.is_empty());
}

#[test]
fn v1_bytes_preserve_focus_and_filename_changes() {
    let entries = vec![
        LocationEntry::new(0, range(7, 100, 110), Some(range(7, 102, 103))),
        LocationEntry::new(20, range(7, 80, 90), Some(range(9, 79, 81))),
        LocationEntry::new(21, range(8, 80, 80), None),
        LocationEntry::new(22, range(8, 81, 82), Some(range(8, 81, 82))),
    ];
    // Fixed V1 bytes also pin backward source deltas and a two-byte entry header.
    let v1 = [
        1, 3, 7, 200, 1, 10, 4, 1, 165, 1, 39, 10, 9, 1, 2, 10, 8, 0, 0, 9, 2, 1, 0, 1,
    ];
    assert_entries(&decode(&v1).unwrap(), &entries);
    let encoded = minicbor_serde::to_vec(Table(entries)).unwrap();
    assert_eq!(encoded, minicbor_serde::to_vec(serde_bytes::Bytes::new(&v1)).unwrap());
}

#[test]
fn integer_boundaries_round_trip() {
    let boundaries = [
        0,
        1,
        15,
        16,
        63,
        64,
        127,
        128,
        16_383,
        16_384,
        i32::MAX.cast_unsigned(),
        1 << 31,
        u32::MAX,
    ];
    let mut entries = Vec::new();
    for start in boundaries {
        for end in boundaries.into_iter().rev() {
            // The codec preserves wrapped ranges and unsorted offsets too.
            entries.push(LocationEntry::new(
                start,
                range(u32::MAX, start, end),
                Some(range(1 << 31, end, start)),
            ));
            entries.push(LocationEntry::new(end, range(0, end, start), None));
        }
    }
    let table = Table(entries);
    let bytes = minicbor_serde::to_vec(&table).unwrap();
    let restored: Table = minicbor_serde::from_slice(&bytes).unwrap();
    assert_entries(&restored.0, &table.0);
}

#[test]
fn legacy_maps_preserve_focus() {
    #[derive(serde::Serialize)]
    struct LegacyEntry {
        #[serde(rename = "B")]
        offset: u32,
        #[serde(rename = "R")]
        range: CodeRange,
        #[serde(rename = "F")]
        focus: Option<CodeRange>,
    }

    let entries = [
        LocationEntry::new(0, range(7, 100, 110), None),
        LocationEntry::new(20, range(7, 80, 90), Some(range(9, 79, 81))),
    ];
    let legacy = entries.each_ref().map(|entry| LegacyEntry {
        offset: entry.bytecode_offset,
        range: entry.range,
        focus: entry.focus,
    });
    let bytes = minicbor_serde::to_vec(&legacy).unwrap();
    let restored: Table = minicbor_serde::from_slice(&bytes).unwrap();
    assert_entries(&restored.0, &entries);
    let bytes = minicbor_serde::to_vec(&restored).unwrap();
    let restored: Table = minicbor_serde::from_slice(&bytes).unwrap();
    assert_entries(&restored.0, &entries);
}

#[test]
fn unknown_format_is_rejected() {
    let bytes = minicbor_serde::to_vec(serde_bytes::Bytes::new(&[255])).unwrap();
    let error = minicbor_serde::from_slice::<Table>(&bytes).err().unwrap();
    assert!(error.to_string().contains("unknown location table format"));
}

fn range(filename: u32, start_byte: u32, end_byte: u32) -> CodeRange {
    CodeRange {
        filename: StringId::from_raw(filename),
        start_byte,
        end_byte,
    }
}

fn assert_entries(actual: &[LocationEntry], expected: &[LocationEntry]) {
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(actual.bytecode_offset, expected.bytecode_offset);
        assert_eq!(actual.range, expected.range);
        assert_eq!(actual.focus, expected.focus);
    }
}
