use super::*;

#[test]
fn random_local_and_inverse_offsets() {
    for text in ["", "ascii", "aé中🐍", "abcdefghüijk中🐍"].map(|s| s.repeat(300)) {
        let index = StringIndex::default();
        let characters: Vec<_> = text.chars().collect();
        let boundaries: Vec<_> = text.char_indices().map(|(byte, _)| byte).chain([text.len()]).collect();
        for n in 0..4096 {
            let target = n * 7919 % (characters.len() + 1);
            assert_eq!(index.byte_offset(&text, target), boundaries[target]);
            assert_eq!(index.char_offset(&text, boundaries[target]), target);
            assert_eq!(
                index.character(&text, i64::try_from(target).unwrap()),
                characters.get(target).copied()
            );
            let negative = -(i64::try_from(target).unwrap()) - 1;
            let expected = characters.len().checked_sub(target + 1).map(|i| characters[i]);
            assert_eq!(index.character(&text, negative), expected);
        }
        assert_eq!(index.character(&text, i64::MIN), None);
        assert_eq!(index.character(&text, i64::MAX), None);
    }
}

#[test]
fn endpoints_do_not_allocate_a_cursor() {
    let text = "a中é🐍".repeat(4096);
    let index = StringIndex::default();
    for _ in 0..100 {
        assert_eq!(index.character(&text, 0), Some('a'));
        assert_eq!(index.character(&text, -1), Some('🐍'));
    }
    assert!(!index.has_cursor());
    for target in 1..text.chars().count() {
        assert!(index.character(&text, i64::try_from(target).unwrap()).is_some());
    }
    assert!(index.has_cursor());
}

#[test]
fn clone_resets_length_and_cursor() {
    let text = "a中é🐍".repeat(4096);
    let index = StringIndex::default();
    assert_eq!(index.char_len(&text), 16384);
    assert_eq!(index.character(&text, 8000), Some('a'));
    assert!(index.has_cursor());
    let cloned = index.clone();
    assert!(index.has_cursor());
    assert_eq!(index.length.get(), 16385);
    assert_eq!(cloned.length.get(), 0);
    assert!(!cloned.has_cursor());
    assert_eq!(cloned.character(&text, 8001), Some('中'));
}
