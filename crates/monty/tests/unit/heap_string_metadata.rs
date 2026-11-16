use super::*;

#[test]
fn ascii_checks_do_not_force_unicode_character_counts() {
    let text = "é".repeat(4096);
    let value = HeapStr::new(text.into_boxed_str());
    assert_eq!(value.char_len.get(), 0);
    assert!(!value.is_ascii());
    assert_eq!(value.char_len.get(), 1);
    assert!(!value.is_ascii());
    assert_eq!(value.char_len.get(), 1);
    assert_eq!(value.char_len(), 4096);
    assert_eq!(value.known_char_len(), Some(4096));
    assert!(!value.is_ascii());

    let ascii = HeapStr::new("a".repeat(4096).into_boxed_str());
    assert!(ascii.is_ascii());
    assert_eq!(ascii.known_char_len(), Some(4096));
    assert_eq!(ascii.char_len(), 4096);
}

#[test]
fn short_heap_strings_do_not_confuse_counts_with_sentinels() {
    for text in ["", "a", "é", "中", "🐍", "ab", "éa", "é🐍"] {
        let count = text.chars().count();
        for value in [
            HeapStr::new(text.into()),
            HeapStr::with_ascii(text.is_ascii(), text.into()),
            HeapStr::with_char_len(text.into(), count),
        ] {
            // Exercise both access orders, including repeated reads of sentinel-sized counts.
            let clone = value.clone();
            for _ in 0..3 {
                assert_eq!(value.char_len(), count, "{text:?}");
                assert_eq!(value.is_ascii(), text.is_ascii(), "{text:?}");
                assert_eq!(clone.is_ascii(), text.is_ascii(), "{text:?}");
                assert_eq!(clone.char_len(), count, "{text:?}");
            }
        }
    }
}

#[test]
fn clones_preserve_metadata_and_snapshots_restart_lazily() {
    let text = "é".repeat(4096);
    let value = HeapStr::with_ascii(false, text.clone().into_boxed_str());
    assert_eq!(value.char_len.get(), 1);
    let clone = value.clone();
    assert_eq!(clone.char_len.get(), 1);
    assert_eq!(clone.char_len(), 4096);
    assert_eq!(value.char_len.get(), 1);
    assert_eq!(value.char_len(), 4096);

    let encoded = minicbor_serde::to_vec(&value).unwrap();
    assert_eq!(encoded, minicbor_serde::to_vec(&text).unwrap());
    let restored: HeapStr = minicbor_serde::from_slice(&encoded).unwrap();
    assert_eq!(restored.char_len.get(), 0);
    assert_eq!(restored.known_char_len(), None);
    assert!(!restored.is_ascii());
    assert_eq!(restored.char_len(), 4096);
    assert_eq!(
        size_of::<HeapStr>(),
        size_of::<(Box<str>, Cell<Option<HashValue>>, usize)>()
    );
}

#[test]
fn known_counts_include_short_unknown_heap_strings() {
    for text in ["", "a", "é", "中", "🐍", "ab", "éa", "abcd", "éé"] {
        let value = HeapStr::new(text.into());
        assert_eq!(value.known_char_len(), Some(text.chars().count()), "{text:?}");
        assert_eq!(value.is_ascii(), text.is_ascii(), "{text:?}");
        assert_eq!(value.char_len(), text.chars().count(), "{text:?}");
    }
    let value = HeapStr::new("éabc".into());
    assert_eq!(value.known_char_len(), None);
    assert_eq!(value.char_len.get(), 0);
}
