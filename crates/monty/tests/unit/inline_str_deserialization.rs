use serde::{
    Deserialize,
    de::value::{BytesDeserializer, Error},
};

use super::InlineStr;

#[test]
fn serde_bytes_deserialize_as_inline_text() {
    let text = "éééééééa";
    let bytes = BytesDeserializer::<Error>::new(text.as_bytes());
    let decoded = InlineStr::deserialize(bytes).unwrap();
    assert_eq!(decoded.as_str(), text);
    assert!(!decoded.is_ascii());
}

#[test]
fn serde_bytes_require_utf8_within_inline_capacity() {
    let invalid = BytesDeserializer::<Error>::new(&[0xff]);
    let error = InlineStr::deserialize(invalid).unwrap_err();
    assert_eq!(error.to_string(), "invalid value: byte array, expected a string");

    let too_long = BytesDeserializer::<Error>::new(b"abcdefghijklmnop");
    let error = InlineStr::deserialize(too_long).unwrap_err();
    assert_eq!(error.to_string(), "inline string exceeds capacity");
}
