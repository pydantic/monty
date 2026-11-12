use super::*;

#[test]
fn character_lengths_survive_text_only_serialization() {
    for text in ["", "a", "ascii literal", "é", "aé🦀", "e\u{301}"] {
        let entry = InternedString::owned(text.to_owned());
        assert_eq!(entry.char_len(), text.chars().count());
        assert_eq!(entry.char_len() == text.len(), text.is_ascii());
        assert_eq!(entry.clone().char_len(), entry.char_len());

        let serialized = serde_json::to_string(&entry).unwrap();
        assert_eq!(serialized, serde_json::to_string(text).unwrap());
        let restored: InternedString = serde_json::from_str(&serialized).unwrap();
        assert_eq!(restored.as_str(), text);
        assert_eq!(restored.char_len(), entry.char_len());
    }

    let interns = Interns::new("");
    for (index, text) in RESERVED_STRS.iter().enumerate() {
        let id = StringId(index.try_into().unwrap());
        assert_eq!(interns.str_char_len(id), text.chars().count());
        assert!(interns.str_is_ascii(id));
    }

    for &value in CORE_STATIC_STRINGS {
        let entry = InternedString::static_string(value);
        assert_eq!(entry.char_len(), entry.as_str().chars().count());
        let id = interns.intern_static(value);
        assert_eq!(interns.str_char_len(id), entry.char_len());
        assert_eq!(interns.str_is_ascii(id), entry.as_str().is_ascii());
    }
}

#[test]
fn uncached_character_lengths_are_recounted() {
    // Exercise the overflow fallback without allocating a multi-gigabyte string.
    for text in ["", "ascii", "aé🦀", "e\u{301}"] {
        let mut entry = InternedString::owned(text.to_owned());
        entry.char_len = None;
        assert_eq!(entry.char_len(), text.chars().count());
        assert_eq!(entry.char_len() == text.len(), text.is_ascii());
    }
}

#[test]
fn cached_character_lengths_use_zero_as_the_sentinel() {
    let empty = InternedString::owned(String::new());
    assert_eq!(empty.char_len, None);
    assert_eq!(empty.char_len(), 0);

    let mut entry = InternedString::owned("é".to_owned());
    assert_eq!(entry.char_len, NonZeroU32::new(1));

    // The largest u32 count is representable; only zero denotes a missing count.
    entry.char_len = NonZeroU32::new(u32::MAX);
    assert_eq!(entry.char_len(), u32::MAX as usize);
    assert_eq!(size_of::<Option<NonZeroU32>>(), size_of::<u32>());
}

/// A larger slot adds its padding cost to all 256 entries reserved by each page.
#[test]
#[cfg(target_pointer_width = "64")]
fn interned_string_slots_stay_compact() {
    assert_eq!(size_of::<Option<InternedString>>(), 40);
}
