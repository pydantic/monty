//! Tests for the structured [`JsonErrorData`] payload attached to
//! `json.JSONDecodeError`, which lets hosts rebuild the real CPython
//! exception (`msg`/`doc`/`pos`/`lineno`/`colno`) instead of a message-only
//! fallback. Host-facing payload behavior is invisible to `test_cases/`, so
//! it is pinned here.

use monty::{CompileOptions, JsonErrorData, MontyException, MontyRun};

/// Runs `code` and returns the resulting `MontyException`.
fn run_exc(code: &str) -> MontyException {
    MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default())
        .unwrap()
        .run_no_limits(vec![])
        .unwrap_err()
}

#[test]
fn json_decode_error_carries_structured_data() {
    let exc = run_exc("import json\njson.loads('[1,\\n2,]')");
    assert_eq!(exc.exc_type().to_string(), "json.JSONDecodeError");
    assert_eq!(
        exc.message(),
        Some("Illegal trailing comma before end of array: line 2 column 2 (char 5)")
    );
    let data = exc.json_data().unwrap();
    assert_eq!(data.msg, "Illegal trailing comma before end of array");
    assert_eq!(data.doc.as_deref(), Some("[1,\n2,]"));
    assert_eq!(data.pos, 5);
    assert_eq!(data.lineno, 2);
    assert_eq!(data.colno, 2);
}

/// `pos`/`colno` count characters like CPython, not `jiter`'s byte offsets —
/// multibyte UTF-8 before the error must not inflate them.
#[test]
fn json_error_positions_count_characters_not_bytes() {
    let exc = run_exc("import json\njson.loads('[\"日本語\", x]')");
    assert_eq!(exc.message(), Some("Expecting value: line 1 column 9 (char 8)"));
    let data = exc.json_data().unwrap();
    assert_eq!(data.doc.as_deref(), Some("[\"日本語\", x]"));
    assert_eq!(data.pos, 8);
    assert_eq!(data.lineno, 1);
    assert_eq!(data.colno, 9);
}

#[test]
fn json_error_data_survives_reraise() {
    let exc = run_exc("import json\ntry:\n    json.loads('nope')\nexcept ValueError as e:\n    raise e");
    assert_eq!(exc.exc_type().to_string(), "json.JSONDecodeError");
    assert!(exc.json_data().is_some());
}

/// Documents larger than `JsonErrorData::MAX_DOC_LEN` are dropped from the
/// payload (the location fields are kept) so a huge input can't be pinned in
/// memory, outside the sandbox's resource tracker, by its exception.
#[test]
fn json_error_doc_omitted_for_huge_documents() {
    let exc = run_exc(&format!(
        "import json\njson.loads('[' + '1,' * {} + 'x')",
        JsonErrorData::MAX_DOC_LEN / 2
    ));
    assert_eq!(exc.exc_type().to_string(), "json.JSONDecodeError");
    let data = exc.json_data().unwrap();
    assert_eq!(data.doc, None);
    assert_eq!(data.pos, JsonErrorData::MAX_DOC_LEN + 1);
}

/// `bytes` input that is not valid UTF-8 cannot be carried as a `str` `doc`;
/// the rest of the payload is still attached.
#[test]
fn json_error_doc_omitted_for_invalid_utf8() {
    let exc = run_exc("import json\njson.loads(b'[1, \\xff]')");
    assert_eq!(exc.exc_type().to_string(), "json.JSONDecodeError");
    let data = exc.json_data().unwrap();
    assert_eq!(data.doc, None);
    assert_eq!(data.pos, 4);
}

/// A manually raised `JSONDecodeError` is message-only — no payload, so hosts
/// fall back to a plain `ValueError`.
#[test]
fn manually_raised_json_decode_error_has_no_data() {
    let exc = run_exc("import json\nraise json.JSONDecodeError('nope')");
    assert_eq!(exc.exc_type().to_string(), "json.JSONDecodeError");
    assert_eq!(exc.json_data(), None);
}
