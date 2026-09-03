//! Tests for `MontyUuid`: construction, uuid4 bit-stamping, parsing,
//! formatting, ordering, and the serde encodings the dump format relies on.

use monty_types::MontyUuid;

const BYTES: [u8; 16] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
];
const CANONICAL: &str = "00010203-0405-0607-0809-0a0b0c0d0e0f";

// === construction ===

#[test]
fn from_bytes_round_trips_as_bytes() {
    assert_eq!(MontyUuid::from_bytes(BYTES).as_bytes(), &BYTES);
}

#[test]
fn from_u128_is_big_endian() {
    let mut expected = [0u8; 16];
    expected[15] = 0x2a;
    assert_eq!(MontyUuid::from_u128(42).as_bytes(), &expected);
    assert_eq!(MontyUuid::from_u128(0).as_bytes(), &[0u8; 16]);
    assert_eq!(MontyUuid::from_u128(u128::MAX).as_bytes(), &[0xff; 16]);
}

#[test]
fn try_from_slice_requires_exactly_16_bytes() {
    assert_eq!(MontyUuid::try_from_slice(&BYTES), Some(MontyUuid::from_bytes(BYTES)));
    assert_eq!(MontyUuid::try_from_slice(&[]), None);
    assert_eq!(MontyUuid::try_from_slice(&BYTES[..15]), None);
    assert_eq!(MontyUuid::try_from_slice(&[0u8; 17]), None);
}

// === uuid4 bit-stamping ===

#[test]
fn from_random_bytes_stamps_version_and_variant() {
    // All-zero and all-one entropy both come out as well-formed uuid4s.
    let zero = MontyUuid::from_random_bytes([0u8; 16]);
    assert_eq!(zero.as_bytes()[6], 0x40, "version nibble must be 4");
    assert_eq!(zero.as_bytes()[8], 0x80, "variant bits must be 10");

    let ones = MontyUuid::from_random_bytes([0xff; 16]);
    assert_eq!(ones.as_bytes()[6], 0x4f, "low version nibble is preserved");
    assert_eq!(ones.as_bytes()[8], 0xbf, "low variant bits are preserved");
}

#[test]
fn from_random_bytes_preserves_entropy_bytes() {
    let stamped = MontyUuid::from_random_bytes(BYTES);
    for (i, (stamped, original)) in stamped.as_bytes().iter().zip(&BYTES).enumerate() {
        if !matches!(i, 6 | 8) {
            assert_eq!(stamped, original, "byte {i} must pass through unchanged");
        }
    }
}

// === Display ===

#[test]
fn display_is_canonical_hyphenated_lowercase() {
    assert_eq!(MontyUuid::from_bytes(BYTES).to_string(), CANONICAL);
    assert_eq!(
        MontyUuid::from_u128(0).to_string(),
        "00000000-0000-0000-0000-000000000000"
    );
    assert_eq!(
        MontyUuid::from_bytes([0xff; 16]).to_string(),
        "ffffffff-ffff-ffff-ffff-ffffffffffff"
    );
    // Hex letters render lowercase.
    assert_eq!(
        MontyUuid::from_bytes([0xab; 16]).to_string(),
        "abababab-abab-abab-abab-abababababab"
    );
}

// === parse ===

#[test]
fn parse_accepts_canonical_and_any_case() {
    let expected = Some(MontyUuid::from_bytes(BYTES));
    assert_eq!(MontyUuid::parse(CANONICAL), expected);
    assert_eq!(MontyUuid::parse(&CANONICAL.to_uppercase()), expected);
    assert_eq!(MontyUuid::parse("00010203-0405-0607-0809-0A0b0C0d0E0f"), expected);
}

#[test]
fn parse_display_round_trips() {
    for v in [0u128, 1, 42, 0xFEED_FACE, u128::MAX] {
        let id = MontyUuid::from_u128(v);
        assert_eq!(MontyUuid::parse(&id.to_string()), Some(id));
    }
    let random = MontyUuid::from_random_bytes(BYTES);
    assert_eq!(MontyUuid::parse(&random.to_string()), Some(random));
}

#[test]
fn parse_rejects_wrong_length() {
    assert_eq!(MontyUuid::parse(""), None);
    assert_eq!(MontyUuid::parse(&CANONICAL[..35]), None);
    assert_eq!(MontyUuid::parse(&format!("{CANONICAL}0")), None);
    // Un-hyphenated 32-digit form is not accepted.
    assert_eq!(MontyUuid::parse(&CANONICAL.replace('-', "")), None);
    // Braced and urn forms are not accepted.
    assert_eq!(MontyUuid::parse(&format!("{{{CANONICAL}}}")), None);
    assert_eq!(MontyUuid::parse(&format!("urn:uuid:{CANONICAL}")), None);
}

#[test]
fn parse_rejects_misplaced_hyphens() {
    // Right length, hyphen shifted one position left of each required slot.
    assert_eq!(MontyUuid::parse("0001020-30405-0607-0809-0a0b0c0d0e0f"), None);
    assert_eq!(MontyUuid::parse("00010203-040-50607-0809-0a0b0c0d0e0f"), None);
    assert_eq!(MontyUuid::parse("00010203-0405-060-70809-0a0b0c0d0e0f"), None);
    assert_eq!(MontyUuid::parse("00010203-0405-0607-080-90a0b0c0d0e0f"), None);
    // All-hyphen and hyphens inside hex groups.
    assert_eq!(MontyUuid::parse("------------------------------------"), None);
    assert_eq!(MontyUuid::parse("00010203-0405-0607-0809-0a0b-c0d0e0f"), None);
}

#[test]
fn parse_rejects_non_hex_characters() {
    assert_eq!(MontyUuid::parse("g0010203-0405-0607-0809-0a0b0c0d0e0f"), None);
    assert_eq!(MontyUuid::parse("00010203-0405-0607-0809-0a0b0c0d0e0g"), None);
    assert_eq!(MontyUuid::parse("00010203-0405-0607-0809-0a0b0c0d0e0 "), None);
    assert_eq!(MontyUuid::parse(" 0010203-0405-0607-0809-0a0b0c0d0e0f"), None);
}

#[test]
fn parse_handles_multibyte_input_without_panicking() {
    // Multibyte UTF-8 (the first two are exactly 36 bytes): byte-indexed
    // scanning must reject these, never slice mid-codepoint or panic.
    assert_eq!(MontyUuid::parse("é0010203-0405-0607-0809-0a0b0c0d0e0"), None);
    assert_eq!(MontyUuid::parse("00010203-0405-0607-0809-0a0b0c0d0é0"), None);
    assert_eq!(MontyUuid::parse("日本語の文字列でちょうど36バイトだよね"), None);
}

// === ordering and hashing ===

#[test]
fn ordering_matches_integer_ordering() {
    // Big-endian byte layout makes lexicographic Ord agree with u128 order.
    let ids: Vec<MontyUuid> = [0u128, 1, 255, 256, u128::MAX]
        .iter()
        .map(|&v| MontyUuid::from_u128(v))
        .collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(sorted, ids);
    assert_eq!(MontyUuid::from_u128(7), MontyUuid::from_u128(7));
    assert_ne!(MontyUuid::from_u128(7), MontyUuid::from_u128(8));
}

// === serde ===

#[test]
fn postcard_encoding_is_exactly_16_bytes() {
    // The dump format relies on the fixed-array encoding: no length prefix.
    let id = MontyUuid::from_bytes(BYTES);
    let encoded = postcard::to_allocvec(&id).unwrap();
    assert_eq!(encoded, BYTES);
    assert_eq!(postcard::from_bytes::<MontyUuid>(&encoded).unwrap(), id);
}

#[test]
fn json_round_trips_as_byte_array() {
    let id = MontyUuid::from_u128(42);
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, "[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,42]");
    assert_eq!(serde_json::from_str::<MontyUuid>(&json).unwrap(), id);
}
