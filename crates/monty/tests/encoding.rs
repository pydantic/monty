//! Regression tests for `str.encode` / `bytes.decode` behavior that
//! intentionally DIVERGES from CPython (see `limitations/encoding.md`), and
//! which therefore cannot live in `test_cases/` — that suite runs every file
//! against CPython too.

use monty::MontyRun;

/// Runs `code` and returns the resulting error's full traceback rendering.
fn run_err(code: &str) -> String {
    MontyRun::new(code.to_owned(), "test.py", vec![])
        .unwrap()
        .run_no_limits(vec![])
        .unwrap_err()
        .to_string()
}

/// Runs `code` and returns its resulting string value.
fn run_str(code: &str) -> String {
    let result = MontyRun::new(code.to_owned(), "test.py", vec![])
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
