//! Regression tests for `str.encode` / `bytes.decode` behavior that
//! intentionally DIVERGES from CPython (see `limitations/encoding.md`), and
//! which therefore cannot live in `test_cases/` — that suite runs every file
//! against CPython too.

use monty::{CompileOptions, MontyException, MontyRun, UnicodeErrorData, UnicodeErrorObject};

/// Runs `code` and returns the resulting error's full traceback rendering.
fn run_err(code: &str) -> String {
    run_exc(code).to_string()
}

/// Runs `code` and returns the resulting `MontyException`.
fn run_exc(code: &str) -> MontyException {
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run_no_limits(vec![])
        .unwrap_err()
}

/// Runs `code` and returns its resulting string value.
fn run_str(code: &str) -> String {
    let result = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run_no_limits(vec![])
        .unwrap();
    result.as_ref().try_into().unwrap()
}

/// CPython's `surrogateescape` decode handler produces lone surrogates, which
/// Monty strings (strict UTF-8) cannot represent, so Monty raises
/// `NotImplementedError` instead. CPython succeeds here (`'h\udce9'`).
#[test]
fn decode_surrogateescape_reports_not_implemented() {
    insta::assert_snapshot!(run_err("b'h\\xe9'.decode('ascii', 'surrogateescape')"), @r#"
    Traceback (most recent call last):
      File "test.py", line 1, in <module>
        b'h\xe9'.decode('ascii', 'surrogateescape')
        ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    NotImplementedError: the 'surrogateescape' error handler is not supported by Monty for decoding: Monty strings cannot contain the lone surrogate characters it produces
    "#);
}

/// `surrogatepass` decodes a CESU-8 surrogate triple to a lone surrogate in
/// CPython (`'\ud800'`); Monty raises `NotImplementedError`. For any *other*
/// invalid UTF-8, `surrogatepass` re-raises the strict error exactly like
/// CPython — that side is covered in `test_cases/codecs__all.py`.
#[test]
fn utf8_surrogatepass_cesu8_reports_not_implemented() {
    insta::assert_snapshot!(run_err("b'\\xed\\xa0\\x80'.decode('utf-8', 'surrogatepass')"), @r#"
    Traceback (most recent call last):
      File "test.py", line 1, in <module>
        b'\xed\xa0\x80'.decode('utf-8', 'surrogatepass')
        ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    NotImplementedError: the 'surrogatepass' error handler is not supported by Monty for decoding: Monty strings cannot contain the lone surrogate characters it produces
    "#);
}

/// A lone UTF-16 surrogate unit under `surrogateescape`/`surrogatepass`:
/// CPython yields `'\ud800'`; Monty raises `NotImplementedError`.
#[test]
fn utf16_surrogate_handlers_report_not_implemented() {
    insta::assert_snapshot!(run_err("b'\\x00\\xd8'.decode('utf-16-le', 'surrogateescape')"), @r#"
    Traceback (most recent call last):
      File "test.py", line 1, in <module>
        b'\x00\xd8'.decode('utf-16-le', 'surrogateescape')
        ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    NotImplementedError: the 'surrogateescape' error handler is not supported by Monty for decoding: Monty strings cannot contain the lone surrogate characters it produces
    "#);
    insta::assert_snapshot!(run_err("b'\\x00\\xd8'.decode('utf-16-le', 'surrogatepass')"), @r#"
    Traceback (most recent call last):
      File "test.py", line 1, in <module>
        b'\x00\xd8'.decode('utf-16-le', 'surrogatepass')
        ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    NotImplementedError: the 'surrogatepass' error handler is not supported by Monty for decoding: Monty strings cannot contain the lone surrogate characters it produces
    "#);
}

/// A UTF-32 surrogate code point under `surrogatepass`: CPython yields
/// `'\ud800'`; Monty raises `NotImplementedError`. (Out-of-range code points
/// re-raise strict like CPython — covered in `test_cases/codecs__all.py`.)
#[test]
fn utf32_surrogatepass_reports_not_implemented() {
    insta::assert_snapshot!(run_err("b'\\x00\\xd8\\x00\\x00'.decode('utf-32-le', 'surrogatepass')"), @r#"
    Traceback (most recent call last):
      File "test.py", line 1, in <module>
        b'\x00\xd8\x00\x00'.decode('utf-32-le', 'surrogatepass')
        ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
    NotImplementedError: the 'surrogatepass' error handler is not supported by Monty for decoding: Monty strings cannot contain the lone surrogate characters it produces
    "#);
}

/// BOM-less bare `utf-16`/`utf-32` decode always assumes little-endian in
/// Monty. CPython assumes the *platform's* byte order, so this asserts the
/// same value CPython would produce on every little-endian host but is kept
/// out of `test_cases/` because it is platform-dependent there.
#[test]
fn bomless_bare_utf16_utf32_decode_defaults_to_little_endian() {
    assert_eq!(run_str("b'a\\x00'.decode('utf-16')"), "a");
    assert_eq!(run_str("b'a\\x00\\x00\\x00'.decode('utf-32')"), "a");
}

/// Codec errors carry CPython's structured constructor fields
/// (`encoding`/`object`/`start`/`end`/`reason`) on the public
/// `MontyException`, so host bindings can rebuild the real
/// `UnicodeDecodeError`/`UnicodeEncodeError`. In-sandbox exceptions expose
/// only `args`, so this can't be tested from `test_cases/`.
#[test]
fn unicode_decode_error_carries_structured_data() {
    let exc = run_exc("b'a\\xffb'.decode()");
    assert_eq!(
        exc.unicode_data(),
        Some(&UnicodeErrorData {
            encoding: "utf-8".to_owned(),
            object: UnicodeErrorObject::Bytes(b"a\xffb".to_vec()),
            start: 1,
            end: 2,
            reason: "invalid start byte".to_owned(),
        })
    );
}

/// Encode errors carry the source string and character (not byte) positions.
#[test]
fn unicode_encode_error_carries_structured_data() {
    let exc = run_exc("'caf\\xe9'.encode('ascii')");
    assert_eq!(
        exc.unicode_data(),
        Some(&UnicodeErrorData {
            encoding: "ascii".to_owned(),
            object: UnicodeErrorObject::Str("café".to_owned()),
            start: 3,
            end: 4,
            reason: "ordinal not in range(128)".to_owned(),
        })
    );
}

/// The payload survives an in-sandbox catch and re-raise.
#[test]
fn unicode_error_data_survives_reraise() {
    let exc = run_exc("try:\n    b'\\xff'.decode()\nexcept ValueError as e:\n    raise e");
    assert_eq!(exc.exc_type().to_string(), "UnicodeDecodeError");
    assert!(exc.unicode_data().is_some());
}

/// Objects larger than `UnicodeErrorData::MAX_OBJECT_LEN` produce no payload
/// (hosts fall back to the message-only form) so a huge input can't be pinned
/// in memory, outside the sandbox's resource tracker, by its exception.
#[test]
fn unicode_error_data_omitted_for_huge_objects() {
    let exc = run_exc("(b'a' * 100_000 + b'\\xff').decode()");
    assert_eq!(exc.exc_type().to_string(), "UnicodeDecodeError");
    assert_eq!(exc.unicode_data(), None);
    // the formatted message is unaffected by the omitted payload
    assert_eq!(
        exc.message(),
        Some("'utf-8' codec can't decode byte 0xff in position 100000: invalid start byte")
    );
}

/// `latin-1` is a real CPython codec that Monty does not implement — pin the
/// `LookupError` so the divergence stays visible and documented.
#[test]
fn latin1_reports_unknown_encoding() {
    insta::assert_snapshot!(run_err("'hi'.encode('latin-1')"), @r#"
    Traceback (most recent call last):
      File "test.py", line 1, in <module>
        'hi'.encode('latin-1')
        ~~~~~~~~~~~~~~~~~~~~~~
    LookupError: unknown encoding: latin-1
    "#);
}
