use super::*;

#[test]
fn source_length_accepts_u32_offsets() {
    for byte_len in [0, 1, u32::MAX as usize] {
        assert!(check_source_length(byte_len, StringId::EMPTY).is_ok());
    }
}

#[test]
#[cfg(target_pointer_width = "64")]
fn source_length_rejects_overflow_without_allocating_source() {
    for byte_len in [u32::MAX as usize + 1, usize::MAX] {
        let error = check_source_length(byte_len, StringId::EMPTY).unwrap_err();
        let ParseError::Syntax { msg, position } = error else {
            panic!("expected SyntaxError");
        };
        assert_eq!(msg, "source exceeds maximum length of 4294967295 bytes");
        assert_eq!(position.filename, StringId::EMPTY);
    }
}
