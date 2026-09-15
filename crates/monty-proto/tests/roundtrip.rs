use std::time::Duration;

use insta::assert_snapshot;
use monty::MontyRun;
use monty_proto::{
    ProtoConvertError, WireArena, ext_result_from_proto, ext_result_to_proto, named_values_from_proto,
    named_values_to_proto, os_call_from_proto, os_call_to_proto, pb,
};
use monty_types::{
    CodeLoc, CompileOptions, ExcData, ExcType, ExtFunctionResult, GetenvArgs, JsonErrorData, MkdirCallArgs, MontyDate,
    MontyDateTime, MontyException, MontyFileHandle, MontyGraph, MontyNode, MontyObject, MontyPath, MontyTime,
    MontyTimeDelta, MontyTimeZone, MontyType, MontyUuid, NameLookupResult, NamedValues, NodeId, OpenCallArgs,
    OsFunctionCall, PathBytesDataArgs, PathStringDataArgs, RenameCallArgs, ResourceLimits, StackFrame,
    UnicodeErrorData, UrandomArgs,
};
use num_bigint::BigInt;
use prost::Message;

/// Asserts `graph` survives `MontyGraph -> wire bytes -> MontyGraph` through
/// the hand-written `WireArena` codec (both directions).
#[track_caller]
fn assert_graph_round_trip(graph: &MontyGraph) {
    let bytes = WireArena::new(graph.clone()).encode_to_vec();
    let back = WireArena::decode(bytes.as_slice())
        .expect("wire bytes -> WireArena failed")
        .into_graph()
        .expect("decoded arena is invalid");
    assert_eq!(&back, graph);
}

/// Asserts `obj` survives the wire as the arena its tree converts to.
#[track_caller]
fn assert_value_round_trip(obj: &MontyObject) {
    assert_graph_round_trip(&obj.graph);
}

#[test]
fn scalar_values_round_trip() {
    assert_value_round_trip(&MontyObject::ellipsis());
    assert_value_round_trip(&MontyObject::none());
    assert_value_round_trip(&MontyObject::bool(true));
    assert_value_round_trip(&MontyObject::bool(false));
    assert_value_round_trip(&MontyObject::int(0));
    assert_value_round_trip(&MontyObject::int(i64::MIN));
    assert_value_round_trip(&MontyObject::int(i64::MAX));
    assert_value_round_trip(&MontyObject::string(String::new()));
    assert_value_round_trip(&MontyObject::string("héllo \u{1F40D}".to_owned()));
    assert_value_round_trip(&MontyObject::bytes(vec![]));
    assert_value_round_trip(&MontyObject::bytes(vec![0, 255, 128]));
    assert_value_round_trip(&MontyObject::path("/mnt/data/file.txt".to_owned()));
}

#[test]
fn float_values_round_trip_bit_exact() {
    // MontyObject's PartialEq compares floats via to_bits, so these assert
    // bit-exact round-trips including NaN and signed zero.
    assert_value_round_trip(&MontyObject::float(0.0));
    assert_value_round_trip(&MontyObject::float(-0.0));
    assert_value_round_trip(&MontyObject::float(f64::NAN));
    assert_value_round_trip(&MontyObject::float(f64::INFINITY));
    assert_value_round_trip(&MontyObject::float(f64::NEG_INFINITY));
    assert_value_round_trip(&MontyObject::float(1.5e300));
}

#[test]
fn bigint_values_round_trip() {
    let huge: BigInt = "123456789012345678901234567890123456789".parse().unwrap();
    assert_value_round_trip(&MontyObject::bigint(huge.clone()));
    assert_value_round_trip(&MontyObject::bigint(-huge));
    assert_value_round_trip(&MontyObject::bigint(BigInt::ZERO));
    assert_value_round_trip(&MontyObject::bigint(BigInt::from(-1)));
}

#[test]
fn container_values_round_trip() {
    assert_value_round_trip(&MontyObject::list([]));
    assert_value_round_trip(&MontyObject::list([
        MontyObject::int(1),
        MontyObject::string("two".to_owned()),
        MontyObject::list([MontyObject::none()]),
    ]));
    assert_value_round_trip(&MontyObject::tuple([MontyObject::bool(true), MontyObject::float(2.5)]));
    assert_value_round_trip(&MontyObject::set([MontyObject::int(1), MontyObject::int(2)]));
    assert_value_round_trip(&MontyObject::frozenset([MontyObject::string("a".to_owned())]));
    // empty dict and a dict with non-string keys (impossible in a proto map)
    assert_value_round_trip(&MontyObject::dict(Vec::new()));
    assert_value_round_trip(&MontyObject::dict([
        (MontyObject::int(1), MontyObject::string("one".to_owned())),
        (
            MontyObject::tuple([MontyObject::int(1), MontyObject::int(2)]),
            MontyObject::none(),
        ),
    ]));
    assert_value_round_trip(&MontyObject::named_tuple(
        "os.stat_result".to_owned(),
        vec!["st_mode".to_owned(), "st_size".to_owned()],
        vec![MontyObject::int(0o644), MontyObject::int(1024)],
    ));
}

#[test]
fn datetime_values_round_trip() {
    assert_value_round_trip(&MontyObject::date(MontyDate {
        year: 2026,
        month: 6,
        day: 11,
    }));
    // naive datetime
    assert_value_round_trip(&MontyObject::datetime(MontyDateTime {
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
    assert_value_round_trip(&MontyObject::datetime(MontyDateTime {
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
    assert_value_round_trip(&MontyObject::timedelta(MontyTimeDelta {
        days: -2,
        seconds: 86399,
        microseconds: 999_999,
    }));
    assert_value_round_trip(&MontyObject::timezone(MontyTimeZone {
        offset_seconds: 19800,
        name: Some("IST".to_owned()),
    }));
    assert_value_round_trip(&MontyObject::timezone(MontyTimeZone {
        offset_seconds: 0,
        name: None,
    }));
}

/// The decode budget is charged `decoded_size`, so every owned string a decoded
/// value carries has to be counted there — the temporal values each hold a
/// caller-supplied timezone name, and the rest of their fields are scalars.
#[test]
fn timezone_names_are_charged_to_the_decode_budget() {
    let name = "z".repeat(500);
    let sizes = |name: Option<String>| {
        [
            MontyObject::datetime(MontyDateTime {
                year: 2026,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
                microsecond: 0,
                offset_seconds: Some(0),
                timezone_name: name.clone(),
            }),
            MontyObject::time(MontyTime {
                hour: 0,
                minute: 0,
                second: 0,
                microsecond: 0,
                offset_seconds: Some(0),
                timezone_name: name.clone(),
                fold: 0,
            }),
            MontyObject::timezone(MontyTimeZone {
                offset_seconds: 0,
                name,
            }),
        ]
        .map(|obj| obj.root_node().decoded_size())
    };
    let named = sizes(Some(name.clone()));
    let unnamed = sizes(None);
    for (named, unnamed) in named.into_iter().zip(unnamed) {
        assert_eq!(named - unnamed, name.len());
    }
}

#[test]
fn exception_and_type_values_round_trip() {
    assert_value_round_trip(&MontyObject::exception(
        ExcType::ValueError,
        Some("bad value".to_owned()),
    ));
    assert_value_round_trip(&MontyObject::exception(ExcType::JsonDecodeError, None));
    assert_value_round_trip(&MontyObject::type_object(MontyType::Int));
    assert_value_round_trip(&MontyObject::type_object(MontyType::DateTime));
    // Qualified name (`collections.deque`) must survive the wire round-trip.
    assert_value_round_trip(&MontyObject::type_object(MontyType::Deque));
    assert_value_round_trip(&MontyObject::type_object(MontyType::Exception(ExcType::KeyError)));
    // Class types round-trip with their uuid, origin and flags.
    assert_value_round_trip(&MontyObject::class_type(
        "Foo".to_owned(),
        MontyUuid::from_u128(0xFEED),
        false,
        false,
        [],
    ));
    assert_value_round_trip(&MontyObject::class_type(
        "Child".to_owned(),
        MontyUuid::from_u128(0xBEEF),
        true,
        true,
        [],
    ));
    let builtin = MontyObject::builtin_function_from_name("len").expect("len is a builtin");
    assert_value_round_trip(&builtin);
    // A dotted builtin name must survive too: `object.__setattr__` is the one
    // whose name is not just its lowercased variant, so it is the only variant
    // that can drift between the strum and serde spellings.
    let dotted =
        MontyObject::builtin_function_from_name("object.__setattr__").expect("object.__setattr__ is a builtin");
    assert_value_round_trip(&dotted);
    assert_eq!(
        serde_json::to_string(dotted.root_node()).expect("serializes"),
        r#"{"BuiltinFunction":"object.__setattr__"}"#
    );
}

#[test]
fn file_handle_values_round_trip() {
    // every mode `open()` can currently produce (`+` modes are rejected by
    // FileMode's parser, so they cannot appear in a real FileHandle)
    for mode in ["r", "rb", "w", "wb", "a", "ab"] {
        assert_value_round_trip(&MontyObject::file_handle(MontyFileHandle {
            path: "/mnt/data/f.bin".to_owned(),
            mode: mode.parse().unwrap(),
            position: 42,
        }));
    }
}

#[test]
fn class_instance_and_function_values_round_trip() {
    assert_value_round_trip(&MontyObject::class_instance(
        MontyObject::class_type("Point".to_owned(), MontyUuid::from_u128(0xDEAD_BEEF), true, true, []),
        MontyUuid::from_u128(0xFEED_FACE),
        vec![
            (MontyObject::string("x".to_owned()), MontyObject::int(1)),
            (MontyObject::string("y".to_owned()), MontyObject::int(2)),
        ],
    ));
    // Sandbox-defined shape: worker-generated ids, non-dataclass, mutable.
    assert_value_round_trip(&MontyObject::class_instance(
        MontyObject::class_type("Widget".to_owned(), MontyUuid::from_u128(3), false, false, []),
        MontyUuid::from_u128(4),
        vec![],
    ));
    // The class branch carries eager class attrs alongside the instance attrs.
    assert_value_round_trip(&MontyObject::class_instance(
        MontyObject::class_type(
            "Square".to_owned(),
            MontyUuid::from_u128(5),
            true,
            false,
            vec![
                (MontyObject::string("SIDES".to_owned()), MontyObject::int(4)),
                (
                    MontyObject::string("KIND".to_owned()),
                    MontyObject::list([MontyObject::string("polygon".to_owned())]),
                ),
            ],
        ),
        MontyUuid::from_u128(6),
        vec![(MontyObject::string("size".to_owned()), MontyObject::int(3))],
    ));
    assert_value_round_trip(&MontyObject::function(
        "fetch".to_owned(),
        Some("fetches a url".to_owned()),
    ));
    assert_value_round_trip(&MontyObject::function("f".to_owned(), None));
}

#[test]
fn repr_and_cycle_round_trip() {
    assert_value_round_trip(&MontyObject::repr("<unrepresentable>".to_owned()));

    // Cycles appear in worker outputs (e.g. a returned cyclic list), so the
    // parent must decode them; produce one via execution and round-trip it.
    // Using one as an *execution input* is rejected by `MontyObject::to_value`.
    let run = MontyRun::new(
        "a = []\na.append(a)\na".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    let cyclic = run.run_no_limits(vec![]).unwrap();
    assert_value_round_trip(&cyclic);
    assert!(matches!(cyclic.as_ref().items().as_deref(), Some([first]) if matches!(first.node(), MontyNode::Cycle(_))));
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
    assert_value_round_trip(&MontyObject::date(MontyDate {
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

/// Multi-line spans render their preview as a pre-computed block with no
/// caret math, and legitimately end on a lower column than they start (a
/// call closed by a hanging `)`), so the same-line column validation must
/// not reject them — regression test for issue #631, where such frames were
/// discarded as "invalid exception payload", replacing the real exception.
#[test]
fn multiline_stack_frame_with_lower_end_column_converts() {
    let frame = pb::StackFrame {
        filename: "main.py".to_owned(),
        start: Some(pb::CodeLoc { line: 4, column: 5 }),
        end: Some(pb::CodeLoc { line: 6, column: 2 }),
        frame_name: None,
        preview_line: Some("r = f(\n    a=1,\n)".to_owned()),
        hide_caret: false,
        hide_frame_name: false,
    };
    let frame = StackFrame::try_from(frame).expect("multi-line span must convert");
    // rendering takes the caret-free block path, so hostile columns are inert
    assert_snapshot!(frame, @r#"
      File "main.py", line 4, in <module>
        r = f(
            a=1,
        )
    "#);
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
        max_duration: Some(Duration::from_millis(1500)),
        max_memory: Some(64 * 1024 * 1024),
        gc_interval: Some(100),
        max_recursion_depth: 50,
        max_suspensions: 7,
    };
    let back = ResourceLimits::from(pb::ResourceLimits::from(&limits));
    assert_eq!(back.max_duration, limits.max_duration);
    assert_eq!(back.max_memory, limits.max_memory);
    assert_eq!(back.gc_interval, limits.gc_interval);
    assert_eq!(back.max_recursion_depth, limits.max_recursion_depth);
    assert_eq!(back.max_suspensions, limits.max_suspensions);
}

#[test]
fn empty_resource_limits_default_recursion_depth() {
    // an all-absent wire message must behave like ResourceLimits::default():
    // unlimited everything except the recursion-depth and suspension defaults
    let back = ResourceLimits::from(pb::ResourceLimits::default());
    let expected = ResourceLimits::default();
    assert_eq!(back.max_duration, expected.max_duration);
    assert_eq!(back.max_memory, expected.max_memory);
    assert_eq!(back.gc_interval, expected.gc_interval);
    assert_eq!(back.max_recursion_depth, expected.max_recursion_depth);
    assert_eq!(back.max_suspensions, expected.max_suspensions);
    assert_eq!(back.max_suspensions, 1000);
}

#[test]
fn ext_results_round_trip() {
    let cases = [
        ExtFunctionResult::Return(MontyObject::int(3)),
        ExtFunctionResult::Error(MontyException::new(ExcType::ValueError, Some("no".to_owned()))),
        ExtFunctionResult::Future(7),
        ExtFunctionResult::NotFound("missing".to_owned()),
    ];
    for case in cases {
        let expected = format!("{case:?}");
        let (proto, values) = ext_result_to_proto(case);
        let back = ext_result_from_proto(proto, values).unwrap();
        // ExtFunctionResult has no PartialEq; compare via Debug
        assert_eq!(format!("{back:?}"), expected);
    }
    // a returned value with no arena to index is rejected
    let (proto, _) = ext_result_to_proto(ExtFunctionResult::Return(MontyObject::int(3)));
    assert!(matches!(
        ext_result_from_proto(proto, None),
        Err(ProtoConvertError::MissingField("values"))
    ));
}

#[test]
fn name_lookup_results_convert() {
    let value = pb::ResumeNameLookup::from(NameLookupResult::from(MontyObject::int(1)));
    assert!(matches!(
        NameLookupResult::try_from(value),
        Ok(NameLookupResult::Value(v)) if v == MontyObject::int(1)
    ));
    // the root must index the arena the message carries
    let out_of_range = pb::ResumeNameLookup {
        values: Some(WireArena::new(MontyObject::int(1).graph)),
        kind: Some(pb::resume_name_lookup::Kind::Value(1)),
    };
    assert!(matches!(
        NameLookupResult::try_from(out_of_range),
        Err(ProtoConvertError::InvalidValue { field: "Arena", .. })
    ));
    let undefined = pb::ResumeNameLookup {
        values: None,
        kind: Some(pb::resume_name_lookup::Kind::Undefined(pb::Unit {})),
    };
    assert!(matches!(
        NameLookupResult::try_from(undefined),
        Ok(NameLookupResult::Undefined)
    ));
    let error = pb::ResumeNameLookup {
        values: None,
        kind: Some(pb::resume_name_lookup::Kind::Error(
            (&MontyException::new(ExcType::KeyError, Some("boom".to_owned()))).into(),
        )),
    };
    let back = NameLookupResult::try_from(error).unwrap();
    let NameLookupResult::Error(exc) = back else {
        panic!("expected Error, got {back:?}");
    };
    assert_eq!(exc.exc_type(), ExcType::KeyError);
    assert_eq!(exc.message(), Some("boom"));
    // an error arm is validated like any exception crossing the wire
    let bogus = pb::ResumeNameLookup {
        values: None,
        kind: Some(pb::resume_name_lookup::Kind::Error(pb::RaisedException {
            exc_type: "NotARealError".to_owned(),
            message: None,
            traceback: vec![],
            data: None,
        })),
    };
    assert!(matches!(
        NameLookupResult::try_from(bogus),
        Err(ProtoConvertError::UnknownExcType(_))
    ));
}

/// The arena is flat, so nesting is not bounded by prost's recursion limit:
/// a chain far deeper than any tree message could carry decodes inside the
/// deepest legitimate frame wrapper (`Request` → `Feed`).
#[test]
fn deep_values_cross_the_wire() {
    let mut graph = MontyGraph::new();
    let mut id = graph.push(MontyNode::Int(1));
    for _ in 0..10_000 {
        id = graph.push(MontyNode::List(vec![id]));
    }
    assert_graph_round_trip(&graph);
    let mut inputs = NamedValues::new();
    inputs.push("v", MontyObject::new(graph, id).unwrap());
    let (refs, values) = named_values_to_proto(inputs.clone());
    let request = pb::ParentRequest {
        kind: Some(pb::parent_request::Kind::Feed(pb::Feed {
            code: String::new(),
            inputs: refs,
            values: Some(values),
            skip_type_check: false,
            cwd: "/work".to_owned(),
        })),
        trace_parent: None,
    };
    let back = pb::ParentRequest::decode(request.encode_to_vec().as_slice()).expect("deep feed decodes");
    let Some(pb::parent_request::Kind::Feed(feed)) = back.kind else {
        panic!("expected a feed");
    };
    assert_eq!(named_values_from_proto(feed.inputs, feed.values).unwrap(), inputs);
}

/// A sub-object referenced twice is one node referenced twice, on the wire as
/// in the arena: the doubling ladder stays linear.
#[test]
fn shared_nodes_stay_shared_on_the_wire() {
    let mut graph = MontyGraph::new();
    let mut x = graph.push(MontyNode::Int(0));
    for _ in 0..20 {
        x = graph.push(MontyNode::List(vec![x, x]));
    }
    assert_eq!(graph.len(), 21);
    let bytes = WireArena::new(graph.clone()).encode_to_vec();
    assert!(bytes.len() < 200, "{} bytes", bytes.len());
    assert_graph_round_trip(&graph);
    let cycle = MontyGraph::from_nodes(vec![
        MontyNode::Cycle("[...]".to_owned()),
        MontyNode::List(vec![NodeId(0)]),
    ]);
    assert_graph_round_trip(&cycle.unwrap());
}

/// Hostile arenas are rejected by `into_graph`, never trusted: an index that
/// is not lower than its holder, a root outside the arena, a class instance
/// whose class is not a class node, and a node with no kind.
#[test]
fn invalid_arenas_are_rejected() {
    let decode = |nodes: Vec<MontyNode>| {
        let bytes = WireArena(nodes).encode_to_vec();
        WireArena::decode(bytes.as_slice())
            .expect("structurally valid")
            .into_graph()
            .map_err(|err| err.to_string())
    };
    assert_eq!(
        decode(vec![MontyNode::List(vec![NodeId(0)])]).unwrap_err(),
        "invalid value for Arena: value node 0 references node 0, which is not below it"
    );
    assert_eq!(
        decode(vec![MontyNode::Int(1), MontyNode::List(vec![NodeId(7)])]).unwrap_err(),
        "invalid value for Arena: value node 1 references node 7, which is not below it"
    );
    assert_eq!(
        decode(vec![
            MontyNode::Int(1),
            MontyNode::ClassInstance {
                class_type: NodeId(0),
                instance_id: MontyUuid::from_u128(1),
                attrs: vec![],
            },
        ])
        .unwrap_err(),
        "invalid value for Arena: class instance node 1 does not point at a class type"
    );
    // an empty arena decodes, but no root can index it
    let empty = pb::Complete {
        value: 0,
        values: Some(WireArena::default()),
    };
    assert_eq!(
        MontyObject::try_from(empty).unwrap_err().to_string(),
        "invalid value for Arena: value root 0 is out of range for an arena of 0 nodes"
    );
    let absent = pb::Complete { value: 0, values: None };
    assert!(matches!(
        MontyObject::try_from(absent),
        Err(ProtoConvertError::MissingField("Complete.values"))
    ));
}

// =============================================================================
// OsCall conversions — the typed wire arms and `OsFunctionCall` map 1:1.
// =============================================================================

/// Asserts `call` survives `OsFunctionCall -> wire bytes -> OsFunctionCall`
/// through the generated `OsCall` message. Compared via `Debug` since
/// `OsFunctionCall` has no `PartialEq`.
#[track_caller]
fn assert_os_call_round_trip(call: OsFunctionCall) {
    let expected = format!("{call:?}");
    let bytes = os_call_to_proto(3, call).encode_to_vec();
    let decoded = pb::OsCall::decode(bytes.as_slice()).expect("wire bytes -> OsCall failed");
    let (call_id, back) = os_call_from_proto(decoded).expect("wire call -> OsFunctionCall failed");
    assert_eq!(call_id, 3);
    assert_eq!(format!("{back:?}"), expected);
}

#[test]
fn os_calls_round_trip_all_variants() {
    let p = || MontyPath::new("/mnt/data/f.txt".to_owned());
    for call in [
        OsFunctionCall::Exists(p()),
        OsFunctionCall::IsFile(p()),
        OsFunctionCall::IsDir(p()),
        OsFunctionCall::IsSymlink(p()),
        OsFunctionCall::ReadText(p()),
        OsFunctionCall::ReadBytes(p()),
        OsFunctionCall::Stat(p()),
        OsFunctionCall::Iterdir(p()),
        OsFunctionCall::Resolve(p()),
        OsFunctionCall::Absolute(p()),
        OsFunctionCall::Unlink(p()),
        OsFunctionCall::Rmdir(p()),
        OsFunctionCall::WriteText(PathStringDataArgs {
            path: p(),
            data: "hello".to_owned(),
        }),
        OsFunctionCall::AppendText(PathStringDataArgs {
            path: p(),
            data: String::new(),
        }),
        OsFunctionCall::WriteBytes(PathBytesDataArgs {
            path: p(),
            data: vec![1, 2, 3],
        }),
        OsFunctionCall::AppendBytes(PathBytesDataArgs {
            path: p(),
            data: vec![],
        }),
        OsFunctionCall::Mkdir(MkdirCallArgs {
            path: p(),
            parents: true,
            exist_ok: false,
        }),
        OsFunctionCall::Rename(RenameCallArgs {
            src: p(),
            dst: MontyPath::new("/mnt/data/g.txt".to_owned()),
        }),
        OsFunctionCall::Getenv(GetenvArgs {
            key: "HOME".to_owned(),
            default: MontyObject::none(),
        }),
        OsFunctionCall::Getenv(GetenvArgs {
            key: "PATH".to_owned(),
            default: MontyObject::list([MontyObject::int(1)]),
        }),
        OsFunctionCall::GetEnviron,
        OsFunctionCall::DateToday,
        OsFunctionCall::DateTimeNow(None),
        OsFunctionCall::DateTimeNow(Some(MontyTimeZone {
            offset_seconds: 3600,
            name: Some("CET".to_owned()),
        })),
        OsFunctionCall::Urandom(UrandomArgs { size: 2496 }),
        OsFunctionCall::Time,
        OsFunctionCall::Sleep(Duration::ZERO),
        OsFunctionCall::Sleep(Duration::from_nanos(1)),
        OsFunctionCall::Sleep(Duration::from_millis(1_500)),
        OsFunctionCall::AsyncSleep(Duration::ZERO),
        OsFunctionCall::AsyncSleep(Duration::from_secs_f64(0.25)),
    ] {
        assert_os_call_round_trip(call);
    }
}

/// The byte count is unsigned on the wire, so the parent cannot see a
/// negative one; a count above `i64::MAX` from a compromised child still
/// converts, reaching the host handler as an exact `BigInt` for its cap to
/// reject.
#[test]
fn os_call_urandom_size_above_i64_converts_exactly() {
    let call = OsFunctionCall::Urandom(UrandomArgs { size: u64::MAX });
    assert_os_call_round_trip(call.clone());
    let args = call.to_args();
    assert_eq!(args.arg(0).unwrap(), MontyObject::bigint(BigInt::from(u64::MAX)));
    assert_eq!(args.args().count(), 1);
    assert_eq!(args.kwargs().count(), 0);
    let args = OsFunctionCall::Urandom(UrandomArgs { size: 2496 }).to_args();
    assert_eq!(args.arg(0).unwrap(), MontyObject::int(2496));
}

/// A child that lies about a sleep length is refused rather than handed on: a
/// host would convert these to its own duration type, and the obvious
/// conversions panic on all three.
#[test]
fn os_call_conversion_rejects_impossible_sleep_lengths() {
    for seconds in [f64::NAN, -1.0, f64::INFINITY, 1e18] {
        let sleep = pb::os_call::Call::Sleep(pb::os_call::Sleep { seconds });
        assert!(
            matches!(
                OsFunctionCall::try_from(sleep),
                Err(ProtoConvertError::InvalidValue {
                    field: "Sleep.seconds",
                    ..
                })
            ),
            "{seconds} should not decode as a sleep length"
        );
        let async_sleep = pb::os_call::Call::AsyncSleep(pb::os_call::AsyncSleep { delay: seconds });
        assert!(
            matches!(
                OsFunctionCall::try_from(async_sleep),
                Err(ProtoConvertError::InvalidValue {
                    field: "AsyncSleep.delay",
                    ..
                })
            ),
            "{seconds} should not decode as an async sleep delay"
        );
    }
}

#[test]
fn os_call_open_round_trips_all_modes() {
    // Only the modes `FromStr` produces — the `+` update modes are reserved
    // and unreachable from user input.
    for mode in ["r", "rb", "w", "wb", "a", "ab"] {
        assert_os_call_round_trip(OsFunctionCall::Open(OpenCallArgs {
            path: MontyPath::new("/mnt/data/f.txt".to_owned()),
            mode: mode.parse().unwrap(),
        }));
    }
}

#[test]
fn os_call_conversion_rejects_invalid_payloads() {
    // A bogus open mode the child could never produce.
    let bad_mode = pb::os_call::Call::Open(pb::os_call::Open {
        path: "/mnt/data/f.txt".to_owned(),
        mode: "q".to_owned(),
    });
    assert!(matches!(
        OsFunctionCall::try_from(bad_mode),
        Err(ProtoConvertError::InvalidFileMode(mode)) if mode == "q"
    ));
    // os.getenv always carries a default (None when the sandbox omitted it),
    // which indexes the envelope's arena: absent, or out of range, is rejected.
    let getenv = |values| pb::OsCall {
        call_id: 1,
        values,
        call: Some(pb::os_call::Call::Getenv(pb::os_call::Getenv {
            key: "HOME".to_owned(),
            default: 1,
        })),
    };
    assert!(matches!(
        os_call_from_proto(getenv(None)),
        Err(ProtoConvertError::MissingField("OsCall.values"))
    ));
    let one_node = WireArena::new(MontyObject::none().graph);
    assert!(matches!(
        os_call_from_proto(getenv(Some(one_node))),
        Err(ProtoConvertError::InvalidValue { field: "Arena", .. })
    ));
}

#[test]
fn shutdown_event_round_trips() {
    let event = pb::ChildEvent {
        kind: Some(pb::child_event::Kind::Shutdown(pb::ShutdownDump {
            dump: Some(vec![1, 2, 3]),
        })),
        ..Default::default()
    };
    let back = pb::ChildEvent::decode(event.encode_to_vec().as_slice()).expect("ShutdownDump event decodes");
    assert_eq!(back, event);
    // a shutdown with nothing to dump (no session yet) also round-trips
    let bare = pb::ChildEvent {
        kind: Some(pb::child_event::Kind::Shutdown(pb::ShutdownDump { dump: None })),
        ..Default::default()
    };
    let back = pb::ChildEvent::decode(bare.encode_to_vec().as_slice()).expect("bare ShutdownDump decodes");
    assert_eq!(back, bare);
}
