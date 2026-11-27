use super::*;

#[test]
fn character_lengths_survive_text_only_serialization() {
    for text in ["", "a", "ascii literal", "é", "aé🦀", "e\u{301}"] {
        let entry = Symbol::owned(Arc::from(text));
        assert_eq!(entry.char_len(), text.chars().count());
        assert_eq!(entry.char_len() == text.len(), text.is_ascii());
        assert_eq!(entry.clone().char_len(), entry.char_len());

        let serialized = serde_json::to_string(&entry).unwrap();
        assert_eq!(serialized, serde_json::to_string(text).unwrap());
        let restored: Symbol = serde_json::from_str(&serialized).unwrap();
        assert_eq!(restored.as_str(), text);
        assert_eq!(restored.char_len(), entry.char_len());
    }

    let symbols = SymbolTable::new("");
    for (index, text) in RESERVED_STRS.iter().enumerate() {
        let id = SymbolId(index.try_into().unwrap());
        assert_eq!(symbols.static_string(id).map(<&str>::from), Some(*text));
        assert_eq!(symbols.str_char_len(id), text.chars().count());
        assert!(symbols.str_is_ascii(id));
    }

    for &value in CORE_STATIC_STRINGS {
        let entry = Symbol::static_string(value);
        assert_eq!(entry.char_len(), entry.as_str().chars().count());
        let id = symbols.get_or_insert_static_symbol(value);
        assert_eq!(symbols.str_char_len(id), entry.char_len());
        assert_eq!(symbols.str_is_ascii(id), entry.as_str().is_ascii());
    }
}

#[test]
fn symbol_table_restore_rejects_duplicate_and_reserved_strings() {
    let cases: &[(&[&str], &str)] = &[
        (&[""], ""),
        (&["x"], "x"),
        (&["splitlines", "splitlines"], "splitlines"),
        (&["ordinary_name", "ordinary_name"], "ordinary_name"),
    ];
    let mut state = serde_json::to_value(SymbolTable::new("")).unwrap();
    for &(strings, rejected) in cases {
        state["strings"] = serde_json::json!(strings);
        let error = serde_json::from_value::<SymbolTable>(state.clone()).unwrap_err();
        assert_eq!(error.to_string(), format!("duplicate or reserved symbol {rejected:?}"));
    }
}

#[test]
#[should_panic(expected = "static entries are indexed by tag")]
fn static_string_has_no_owned_lookup_key() {
    Symbol::static_string(StaticStrings::Module).owned_text();
}

#[test]
fn uncached_character_lengths_are_recounted() {
    // Exercise the overflow fallback without allocating a multi-gigabyte string.
    for text in ["", "ascii", "aé🦀", "e\u{301}"] {
        let mut entry = Symbol::owned(Arc::from(text));
        entry.char_len = None;
        assert_eq!(entry.char_len(), text.chars().count());
        assert_eq!(entry.char_len() == text.len(), text.is_ascii());
    }
}

#[test]
fn cached_character_lengths_use_zero_as_the_sentinel() {
    let empty = Symbol::owned(Arc::from(""));
    assert_eq!(empty.char_len, None);
    assert_eq!(empty.char_len(), 0);

    let mut entry = Symbol::owned(Arc::from("é"));
    assert_eq!(entry.char_len, NonZeroU32::new(1));

    // The largest u32 count is representable; only zero denotes a missing count.
    entry.char_len = NonZeroU32::new(u32::MAX);
    assert_eq!(entry.char_len(), u32::MAX as usize);
    assert_eq!(size_of::<Option<NonZeroU32>>(), size_of::<u32>());
}

/// A larger slot adds its padding cost to all 256 entries reserved by each page.
#[test]
#[cfg(target_pointer_width = "64")]
fn symbol_slots_stay_compact() {
    assert_eq!(size_of::<Option<Symbol>>(), 40);
}
