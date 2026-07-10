use std::time::Duration;

use monty::{
    CodeLoc, DictPairs, ExcData, ExcType, ExtFunctionResult, JsonErrorData, MontyDate, MontyDateTime, MontyException,
    MontyFileHandle, MontyObject, MontyRun, MontyTimeDelta, MontyTimeZone, MontyType, NameLookupResult, ResourceLimits,
    StackFrame, UnicodeErrorData,
};
use monty_proto::{MAX_VALUE_DEPTH, ProtoConvertError, WireObject, exceeds_max_value_depth, pb};
use num_bigint::BigInt;
use prost::Message;

/// Asserts `obj` survives `MontyObject -> wire bytes -> MontyObject` through
/// the hand-written `WireObject` codec (both directions).
#[track_caller]
fn assert_value_round_trip(obj: &MontyObject) {
    let bytes = WireObject::new(obj.clone()).encode_to_vec();
    let back = WireObject::decode(bytes.as_slice())
        .expect("wire bytes -> WireObject failed")
        .into_object()
        .expect("decoded value has no kind");
    assert_eq!(&back, obj);
}

#[test]
fn scalar_values_round_trip() {
    assert_value_round_trip(&MontyObject::Ellipsis);
    assert_value_round_trip(&MontyObject::None);
    assert_value_round_trip(&MontyObject::Bool(true));
    assert_value_round_trip(&MontyObject::Bool(false));
    assert_value_round_trip(&MontyObject::Int(0));
    assert_value_round_trip(&MontyObject::Int(i64::MIN));
    assert_value_round_trip(&MontyObject::Int(i64::MAX));
    assert_value_round_trip(&MontyObject::String(String::new()));
    assert_value_round_trip(&MontyObject::String("héllo \u{1F40D}".to_owned()));
    assert_value_round_trip(&MontyObject::Bytes(vec![]));
    assert_value_round_trip(&MontyObject::Bytes(vec![0, 255, 128]));
    assert_value_round_trip(&MontyObject::Path("/mnt/data/file.txt".to_owned()));
}

#[test]
fn float_values_round_trip_bit_exact() {
    // MontyObject's PartialEq compares floats via to_bits, so these assert
    // bit-exact round-trips including NaN and signed zero.
    assert_value_round_trip(&MontyObject::Float(0.0));
    assert_value_round_trip(&MontyObject::Float(-0.0));
    assert_value_round_trip(&MontyObject::Float(f64::NAN));
    assert_value_round_trip(&MontyObject::Float(f64::INFINITY));
    assert_value_round_trip(&MontyObject::Float(f64::NEG_INFINITY));
    assert_value_round_trip(&MontyObject::Float(1.5e300));
}

#[test]
fn bigint_values_round_trip() {
    let huge: BigInt = "123456789012345678901234567890123456789".parse().unwrap();
    assert_value_round_trip(&MontyObject::BigInt(huge.clone()));
    assert_value_round_trip(&MontyObject::BigInt(-huge));
    assert_value_round_trip(&MontyObject::BigInt(BigInt::ZERO));
    assert_value_round_trip(&MontyObject::BigInt(BigInt::from(-1)));
}

#[test]
fn container_values_round_trip() {
    assert_value_round_trip(&MontyObject::List(vec![]));
    assert_value_round_trip(&MontyObject::List(vec![
        MontyObject::Int(1),
        MontyObject::String("two".to_owned()),
        MontyObject::List(vec![MontyObject::None]),
    ]));
    assert_value_round_trip(&MontyObject::Tuple(vec![
        MontyObject::Bool(true),
        MontyObject::Float(2.5),
    ]));
    assert_value_round_trip(&MontyObject::Set(vec![MontyObject::Int(1), MontyObject::Int(2)]));
    assert_value_round_trip(&MontyObject::FrozenSet(vec![MontyObject::String("a".to_owned())]));
    // empty dict and a dict with non-string keys (impossible in a proto map)
    assert_value_round_trip(&MontyObject::dict(Vec::new()));
    assert_value_round_trip(&MontyObject::dict(vec![
        (MontyObject::Int(1), MontyObject::String("one".to_owned())),
        (
            MontyObject::Tuple(vec![MontyObject::Int(1), MontyObject::Int(2)]),
            MontyObject::None,
        ),
    ]));
    assert_value_round_trip(&MontyObject::NamedTuple {
        type_name: "os.stat_result".to_owned(),
        field_names: vec!["st_mode".to_owned(), "st_size".to_owned()],
        values: vec![MontyObject::Int(0o644), MontyObject::Int(1024)],
    });
}

#[test]
fn datetime_values_round_trip() {
    assert_value_round_trip(&MontyObject::Date(MontyDate {
        year: 2026,
        month: 6,
        day: 11,
    }));
    // naive datetime
    assert_value_round_trip(&MontyObject::DateTime(MontyDateTime {
        year: 2026,
        month: 6,
        day: 11,
        hour: 23,
        minute: 59,
        second: 58,
        microsecond: 999_999,
        offset_seconds: None,
        timezone_name: None,
    }));
    // aware datetime with a named zone
    assert_value_round_trip(&MontyObject::DateTime(MontyDateTime {
        year: 1999,
        month: 1,
        day: 2,
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        offset_seconds: Some(-3600),
        timezone_name: Some("UTC-01:00".to_owned()),
    }));
    assert_value_round_trip(&MontyObject::TimeDelta(MontyTimeDelta {
        days: -2,
        seconds: 86399,
        microseconds: 999_999,
    }));
    assert_value_round_trip(&MontyObject::TimeZone(MontyTimeZone {
        offset_seconds: 19800,
        name: Some("IST".to_owned()),
    }));
    assert_value_round_trip(&MontyObject::TimeZone(MontyTimeZone {
        offset_seconds: 0,
        name: None,
    }));
}

#[test]
fn exception_and_type_values_round_trip() {
    assert_value_round_trip(&MontyObject::Exception {
        exc_type: ExcType::ValueError,
        arg: Some("bad value".to_owned()),
    });
    assert_value_round_trip(&MontyObject::Exception {
        exc_type: ExcType::JsonDecodeError,
        arg: None,
    });
    assert_value_round_trip(&MontyObject::Type(MontyType::Int));
    assert_value_round_trip(&MontyObject::Type(MontyType::DateTime));
    assert_value_round_trip(&MontyObject::Type(MontyType::Exception(ExcType::KeyError)));
    assert_value_round_trip(&MontyObject::Type(MontyType::Instance("Foo".to_owned())));
    let builtin = MontyObject::builtin_function_from_name("len").expect("len is a builtin");
    assert_value_round_trip(&builtin);
}

#[test]
fn file_handle_values_round_trip() {
    // every mode `open()` can currently produce (`+` modes are rejected by
    // FileMode's parser, so they cannot appear in a real FileHandle)
    for mode in ["r", "rb", "w", "wb", "a", "ab"] {
        assert_value_round_trip(&MontyObject::FileHandle(MontyFileHandle {
            path: "/mnt/data/f.bin".to_owned(),
            mode: mode.parse().unwrap(),
            position: 42,
        }));
    }
}

#[test]
fn dataclass_and_function_values_round_trip() {
    assert_value_round_trip(&MontyObject::Dataclass {
        name: "Point".to_owned(),
        type_id: 0xDEAD_BEEF,
        field_names: vec!["x".to_owned(), "y".to_owned()],
        attrs: DictPairs::from(vec![
            (MontyObject::String("x".to_owned()), MontyObject::Int(1)),
            (MontyObject::String("y".to_owned()), MontyObject::Int(2)),
        ]),
        frozen: true,
    });
    assert_value_round_trip(&MontyObject::Function {
        name: "fetch".to_owned(),
        docstring: Some("fetches a url".to_owned()),
    });
    assert_value_round_trip(&MontyObject::Function {
        name: "f".to_owned(),
        docstring: None,
    });
}

#[test]
fn repr_and_cycle_round_trip() {
    assert_value_round_trip(&MontyObject::Repr("<unrepresentable>".to_owned()));

    // Cycles appear in worker outputs (e.g. a returned cyclic list), so the
    // parent must decode them; produce one via execution and round-trip it.
    // Using one as an *execution input* is rejected by `MontyObject::to_value`.
    let run = MontyRun::new("a = []\na.append(a)\na".to_owned(), "test.py", vec![]).unwrap();
    let cyclic = run.run_no_limits(vec![]).unwrap();
    assert_value_round_trip(&cyclic);
    assert!(matches!(&cyclic, MontyObject::List(items) if matches!(items[0], MontyObject::Cycle(_, _))));
}

// NOTE: rejection of semantically invalid wire values (bad dates, unknown
// enum names, missing oneofs, ...) now happens *during decode* and is tested
// in `tests/differential.rs`, which uses the fully-generated oracle to craft
// hostile frames the hand-written codec must reject.

/// A leap-day date is the trickiest valid temporal value — keep it as a
/// round-trip check here (rejection of invalid dates lives in the
/// differential tests).
#[test]
fn leap_day_round_trips() {
    assert_value_round_trip(&MontyObject::Date(MontyDate {
        year: 2024,
        month: 2,
        day: 29, // 2024 is a leap year
    }));
}

/// `StackFrame`'s `Display` derives caret padding/width from the columns, so
/// frames whose columns underflow the caret subtraction or point far outside
/// the preview line (panic / unbounded-allocation vectors when rendering a
/// hostile traceback) must be rejected at the conversion boundary.
#[test]
fn invalid_stack_frame_coordinates_are_rejected() {
    let frame = |start_column, end_column| pb::StackFrame {
        filename: "main.py".to_owned(),
        start: Some(pb::CodeLoc {
            line: 1,
            column: start_column,
        }),
        end: Some(pb::CodeLoc {
            line: 1,
            column: end_column,
        }),
        frame_name: None,
        preview_line: Some("foo()".to_owned()),
        hide_caret: false,
        hide_frame_name: false,
    };
    // end before start would underflow the caret-width subtraction
    assert!(matches!(
        StackFrame::try_from(frame(5, 1)),
        Err(ProtoConvertError::InvalidValue {
            field: "StackFrame.end.column",
            ..
        })
    ));
    // a column far beyond the 5-character preview would allocate a
    // pathologically wide caret line
    assert!(matches!(
        StackFrame::try_from(frame(1, u32::MAX)),
        Err(ProtoConvertError::InvalidValue {
            field: "StackFrame.end.column",
            ..
        })
    ));
    StackFrame::try_from(frame(1, 6)).expect("in-range columns must convert");
}

#[test]
fn exceptions_round_trip_with_traceback() {
    let frames = vec![
        StackFrame {
            filename: "main.py".to_owned(),
            start: CodeLoc { line: 4, column: 1 },
            end: CodeLoc { line: 4, column: 6 },
            frame_name: None,
            preview_line: Some("foo()".into()),
            hide_caret: false,
            hide_frame_name: false,
        },
        StackFrame {
            filename: "main.py".to_owned(),
            start: CodeLoc { line: 2, column: 5 },
            end: CodeLoc { line: 2, column: 30 },
            frame_name: Some("foo".to_owned()),
            preview_line: Some("    raise ValueError('oops')".into()),
            hide_caret: true,
            hide_frame_name: false,
        },
    ];
    let exc = MontyException::with_traceback(ExcType::ValueError, Some("oops".to_owned()), frames);
    let proto = pb::RaisedException::from(&exc);
    let back = MontyException::try_from(proto).expect("proto -> MontyException failed");
    assert_eq!(back, exc);
    // the rendered traceback (the user-visible artifact) must be identical
    assert_eq!(back.to_string(), exc.to_string());
}

#[test]
fn exception_without_traceback_round_trips() {
    let exc = MontyException::new(ExcType::TypeError, None);
    let back = MontyException::try_from(pb::RaisedException::from(&exc)).unwrap();
    assert_eq!(back, exc);
}

/// Builds a wire `UnicodeDecodeError` whose payload fields a byzantine child
/// controls, for probing the receive-side sanitizer.
fn unicode_exception(encoding: String, object: Vec<u8>, start: u64, end: u64, reason: String) -> pb::RaisedException {
    pb::RaisedException {
        exc_type: "UnicodeDecodeError".to_owned(),
        message: Some("boom".to_owned()),
        traceback: vec![],
        data: Some(pb::ExcData {
            kind: Some(pb::exc_data::Kind::Unicode(pb::UnicodeErrorData {
                encoding,
                object: Some(pb::unicode_error_data::Object::ObjectBytes(object)),
                start,
                end,
                reason,
            })),
        }),
    }
}

#[test]
fn bogus_unicode_payloads_are_dropped_not_trusted() {
    let oversized = "x".repeat(UnicodeErrorData::MAX_OBJECT_LEN + 1);
    // Every rejected payload still converts — only the structured data is
    // dropped, since a hostile child must not be able to block error reporting.
    let bogus = [
        unicode_exception(oversized.clone(), vec![0xFF], 0, 1, "reason".to_owned()),
        unicode_exception("utf-8".to_owned(), vec![0xFF], 0, 1, oversized.clone()),
        unicode_exception("utf-8".to_owned(), oversized.into_bytes(), 0, 1, "reason".to_owned()),
        // empty and inverted ranges, and a range beyond the object
        unicode_exception("utf-8".to_owned(), vec![0xFF], 1, 1, "reason".to_owned()),
        unicode_exception("utf-8".to_owned(), vec![0xFF], 1, 0, "reason".to_owned()),
        unicode_exception("utf-8".to_owned(), vec![0xFF], 0, 2, "reason".to_owned()),
    ];
    for proto in bogus {
        let back = MontyException::try_from(proto).expect("malformed payload must not block conversion");
        assert_eq!(back.data(), &ExcData::None);
    }

    // An in-bounds payload survives sanitization intact.
    let back = MontyException::try_from(unicode_exception(
        "utf-8".to_owned(),
        vec![0x61, 0xFF],
        1,
        2,
        "invalid start byte".to_owned(),
    ))
    .unwrap();
    assert!(matches!(back.data(), ExcData::Unicode(data) if data.start == 1 && data.end == 2));
}

#[test]
fn json_error_payload_round_trips() {
    let data = JsonErrorData {
        msg: "Expecting value".to_owned(),
        doc: Some("[1,\n2,]".to_owned()),
        pos: 6,
        lineno: 2,
        colno: 3,
    };
    let exc = MontyException::new(
        ExcType::JsonDecodeError,
        Some("Expecting value: line 2 column 3 (char 6)".to_owned()),
    )
    .with_data(ExcData::Json(Box::new(data)));
    let back = MontyException::try_from(pb::RaisedException::from(&exc)).unwrap();
    assert_eq!(back, exc);

    // A payload without a document (dropped for oversized inputs) also survives.
    let exc = MontyException::new(ExcType::JsonDecodeError, Some("boom".to_owned())).with_data(ExcData::Json(
        Box::new(JsonErrorData {
            msg: "Expecting value".to_owned(),
            doc: None,
            pos: 100_000,
            lineno: 5,
            colno: 2,
        }),
    ));
    let back = MontyException::try_from(pb::RaisedException::from(&exc)).unwrap();
    assert_eq!(back, exc);
}

/// Builds a wire `json.JSONDecodeError` whose payload fields a byzantine
/// child controls, for probing the receive-side sanitizer.
fn json_exception(msg: String, doc: Option<String>, pos: u64, lineno: u64, colno: u64) -> pb::RaisedException {
    pb::RaisedException {
        exc_type: "json.JSONDecodeError".to_owned(),
        message: Some("boom".to_owned()),
        traceback: vec![],
        data: Some(pb::ExcData {
            kind: Some(pb::exc_data::Kind::Json(pb::JsonErrorData {
                msg,
                doc,
                pos,
                lineno,
                colno,
            })),
        }),
    }
}

#[test]
fn bogus_json_payloads_are_dropped_not_trusted() {
    let oversized = "x".repeat(JsonErrorData::MAX_DOC_LEN + 1);
    // As with unicode payloads, rejected data must not block conversion.
    let bogus = [
        json_exception(oversized.clone(), None, 0, 1, 1),
        json_exception("msg".to_owned(), Some(oversized), 0, 1, 1),
        // 0 is not a valid 1-based line/column
        json_exception("msg".to_owned(), None, 0, 0, 1),
        json_exception("msg".to_owned(), None, 0, 1, 0),
        // pos beyond the document
        json_exception("msg".to_owned(), Some("[]".to_owned()), 3, 1, 1),
    ];
    for proto in bogus {
        let back = MontyException::try_from(proto).expect("malformed payload must not block conversion");
        assert_eq!(back.data(), &ExcData::None);
    }

    // An in-bounds payload survives sanitization intact; pos may equal the
    // document length (errors at end of input).
    let back = MontyException::try_from(json_exception(
        "Expecting value".to_owned(),
        Some("[1,".to_owned()),
        3,
        1,
        4,
    ))
    .unwrap();
    assert!(matches!(back.data(), ExcData::Json(data) if data.pos == 3 && data.colno == 4));
}

#[test]
fn resource_limits_round_trip() {
    let limits = ResourceLimits {
        max_allocations: Some(10_000),
        max_duration: Some(Duration::from_millis(1500)),
        max_memory: Some(64 * 1024 * 1024),
        gc_interval: Some(100),
        max_recursion_depth: Some(50),
    };
    let back = ResourceLimits::from(pb::ResourceLimits::from(&limits));
    assert_eq!(back.max_allocations, limits.max_allocations);
    assert_eq!(back.max_duration, limits.max_duration);
    assert_eq!(back.max_memory, limits.max_memory);
    assert_eq!(back.gc_interval, limits.gc_interval);
    assert_eq!(back.max_recursion_depth, limits.max_recursion_depth);
}

#[test]
fn empty_resource_limits_default_recursion_depth() {
    // an all-absent wire message must behave like ResourceLimits::new():
    // unlimited everything except the standard recursion-depth default
    let back = ResourceLimits::from(pb::ResourceLimits::default());
    let expected = ResourceLimits::new();
    assert_eq!(back.max_allocations, expected.max_allocations);
    assert_eq!(back.max_duration, expected.max_duration);
    assert_eq!(back.max_memory, expected.max_memory);
    assert_eq!(back.gc_interval, expected.gc_interval);
    assert_eq!(back.max_recursion_depth, expected.max_recursion_depth);
}

#[test]
fn ext_results_round_trip() {
    let cases = [
        ExtFunctionResult::Return(MontyObject::Int(3)),
        ExtFunctionResult::Error(MontyException::new(ExcType::ValueError, Some("no".to_owned()))),
        ExtFunctionResult::Future(7),
        ExtFunctionResult::NotFound("missing".to_owned()),
    ];
    for case in cases {
        let expected = format!("{case:?}");
        let proto = pb::ExtFunctionResult::from(case);
        let back = ExtFunctionResult::try_from(proto).unwrap();
        // ExtFunctionResult has no PartialEq; compare via Debug
        assert_eq!(format!("{back:?}"), expected);
    }
}

#[test]
fn name_lookup_results_convert() {
    let value = pb::ResumeNameLookup {
        kind: Some(pb::resume_name_lookup::Kind::Value(WireObject::new(MontyObject::Int(
            1,
        )))),
    };
    assert!(matches!(
        NameLookupResult::try_from(value),
        Ok(NameLookupResult::Value(MontyObject::Int(1)))
    ));
    let undefined = pb::ResumeNameLookup {
        kind: Some(pb::resume_name_lookup::Kind::Undefined(pb::Unit {})),
    };
    assert!(matches!(
        NameLookupResult::try_from(undefined),
        Ok(NameLookupResult::Undefined)
    ));
}

/// Deeply nested values: encoding works at depths a sandbox can plausibly
/// produce, and prost's decode recursion limit bounds what a malicious peer
/// can make the receiver process.
#[test]
fn nested_value_round_trip() {
    let mut value = MontyObject::Int(1);
    for _ in 0..20 {
        value = MontyObject::List(vec![value]);
    }
    assert_value_round_trip(&value);
}

/// `Int(1)` nested in `depth` levels of list (2 proto levels per level).
fn nest_list(depth: usize) -> MontyObject {
    (0..depth).fold(MontyObject::Int(1), |inner, _| MontyObject::List(vec![inner]))
}

/// `Int(1)` nested in `depth` levels of single-entry dict (3 proto levels per
/// level: `MontyObject` + `Dict` + `Pair`).
fn nest_dict(depth: usize) -> MontyObject {
    (0..depth).fold(MontyObject::Int(1), |inner, _| {
        MontyObject::dict(vec![(MontyObject::String("k".to_owned()), inner)])
    })
}

/// `Int(1)` nested in `depth` levels of single-field dataclass (4 proto
/// levels per level: `MontyObject` + `Dataclass` + `Dict` + `Pair`).
fn nest_dataclass(depth: usize) -> MontyObject {
    (0..depth).fold(MontyObject::Int(1), |inner, _| MontyObject::Dataclass {
        name: "D".to_owned(),
        type_id: 1,
        field_names: vec!["f".to_owned()],
        attrs: DictPairs::from(vec![(MontyObject::String("f".to_owned()), inner)]),
        frozen: false,
    })
}

/// Whether `value` decodes when shipped inside the deepest legitimate frame
/// wrapper chain (`Request` → `Feed` → `NamedValue`).
fn decodes_in_frame(value: &MontyObject) -> bool {
    let request = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: String::new(),
            inputs: vec![pb::NamedValue {
                name: "v".to_owned(),
                value: Some(WireObject::new(value.clone())),
            }],
            mounts: vec![],
            skip_type_check: false,
        })),
    };
    pb::ParentRequest::decode(request.encode_to_vec().as_slice()).is_ok()
}

/// The sender-side depth check must agree exactly with what the receiver can
/// decode, for every container shape: dicts and dataclasses consume more of
/// prost's recursion budget per level than lists, so a uniform per-container
/// budget would pass values that then fail to decode (and kill the worker as
/// a protocol failure instead of raising a clean depth error).
#[test]
fn depth_check_matches_frame_decodability() {
    /// One container shape: name, nesting builder, deepest depth that must pass.
    type DepthCase = (&'static str, fn(usize) -> MontyObject, usize);
    let cases: [DepthCase; 3] = [
        ("list", nest_list, MAX_VALUE_DEPTH), // 48: 2 proto levels each
        ("dict", nest_dict, 32),              // 3 proto levels each
        ("dataclass", nest_dataclass, 24),    // 4 proto levels each
    ];
    for (shape, build, max_depth) in cases {
        let deepest = build(max_depth);
        assert!(
            !exceeds_max_value_depth(&deepest),
            "{shape} nested {max_depth} deep should pass the depth check"
        );
        assert!(
            decodes_in_frame(&deepest),
            "{shape} nested {max_depth} deep should decode inside a frame"
        );
        let too_deep = build(max_depth + 1);
        assert!(
            exceeds_max_value_depth(&too_deep),
            "{shape} nested {} deep should fail the depth check",
            max_depth + 1
        );
        assert!(
            !decodes_in_frame(&too_deep),
            "{shape} nested {} deep should fail to decode inside a frame",
            max_depth + 1
        );
    }
}
