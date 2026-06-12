use std::time::Duration;

use monty::{
    CodeLoc, DictPairs, ExcType, ExtFunctionResult, MontyDate, MontyDateTime, MontyException, MontyFileHandle,
    MontyObject, MontyRun, MontyTimeDelta, MontyTimeZone, NameLookupResult, ResourceLimits, StackFrame, Type,
};
use monty_proto::{ProtoConvertError, pb};
use num_bigint::BigInt;

/// Asserts `obj` survives `MontyObject -> pb::MontyValue -> MontyObject`.
#[track_caller]
fn assert_value_round_trip(obj: &MontyObject) {
    let proto = pb::MontyValue::from(obj);
    let back = MontyObject::try_from(proto).expect("proto -> MontyObject failed");
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
    assert_value_round_trip(&MontyObject::Type(Type::Int));
    assert_value_round_trip(&MontyObject::Type(Type::DateTime));
    assert_value_round_trip(&MontyObject::Type(Type::Exception(ExcType::KeyError)));
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

#[test]
fn invalid_values_are_rejected() {
    // unknown exception type name
    let bad_exc = pb::MontyValue {
        kind: Some(pb::monty_value::Kind::Exception(pb::ExceptionValue {
            exc_type: "NotARealError".to_owned(),
            arg: None,
        })),
    };
    assert!(matches!(
        MontyObject::try_from(bad_exc),
        Err(ProtoConvertError::UnknownExcType(name)) if name == "NotARealError"
    ));

    // month out of u8 range
    let bad_date = pb::MontyValue {
        kind: Some(pb::monty_value::Kind::Date(pb::DateValue {
            year: 2026,
            month: 4096,
            day: 1,
        })),
    };
    assert!(matches!(
        MontyObject::try_from(bad_date),
        Err(ProtoConvertError::InvalidValue {
            field: "DateValue.month",
            ..
        })
    ));

    // timezone_name without offset_seconds
    let bad_dt = pb::MontyValue {
        kind: Some(pb::monty_value::Kind::Datetime(pb::DateTimeValue {
            year: 2026,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            offset_seconds: None,
            timezone_name: Some("UTC".to_owned()),
        })),
    };
    assert!(matches!(
        MontyObject::try_from(bad_dt),
        Err(ProtoConvertError::InvalidValue { .. })
    ));

    // missing oneof
    let empty = pb::MontyValue { kind: None };
    assert!(matches!(
        MontyObject::try_from(empty),
        Err(ProtoConvertError::MissingField("MontyValue.kind"))
    ));

    // update file modes are not yet supported by monty's parser
    let bad_mode = pb::MontyValue {
        kind: Some(pb::monty_value::Kind::FileHandle(pb::FileHandleValue {
            path: "/f".to_owned(),
            mode: "r+".to_owned(),
            position: 0,
        })),
    };
    assert!(matches!(
        MontyObject::try_from(bad_mode),
        Err(ProtoConvertError::InvalidFileMode(mode)) if mode == "r+"
    ));
}

/// The wire is untrusted: temporal values that fit their integer fields but
/// violate the semantic invariants documented on `MontyDate`/`MontyDateTime`/
/// `MontyTimeDelta` must be rejected at the conversion boundary.
#[test]
fn out_of_range_temporal_values_are_rejected() {
    let date = |year, month, day| pb::MontyValue {
        kind: Some(pb::monty_value::Kind::Date(pb::DateValue { year, month, day })),
    };
    let rejected_as = |value: pb::MontyValue, expected_field: &str| {
        matches!(
            MontyObject::try_from(value),
            Err(ProtoConvertError::InvalidValue { field, .. }) if field == expected_field
        )
    };
    assert!(rejected_as(date(0, 1, 1), "DateValue.year"));
    assert!(rejected_as(date(10_000, 1, 1), "DateValue.year"));
    assert!(rejected_as(date(2026, 0, 1), "DateValue.month"));
    assert!(rejected_as(date(2026, 13, 1), "DateValue.month"));
    assert!(rejected_as(date(2026, 2, 0), "DateValue.day"));
    assert!(rejected_as(date(2026, 2, 29), "DateValue.day")); // 2026 is not a leap year
    assert!(rejected_as(date(2025, 4, 31), "DateValue.day"));
    assert_value_round_trip(&MontyObject::Date(MontyDate {
        year: 2024,
        month: 2,
        day: 29, // 2024 is a leap year
    }));

    let datetime_with_hour = |hour| pb::MontyValue {
        kind: Some(pb::monty_value::Kind::Datetime(pb::DateTimeValue {
            year: 2026,
            month: 1,
            day: 1,
            hour,
            minute: 0,
            second: 0,
            microsecond: 0,
            offset_seconds: None,
            timezone_name: None,
        })),
    };
    assert!(rejected_as(datetime_with_hour(24), "DateTimeValue.hour"));

    let timedelta = |seconds, microseconds| pb::MontyValue {
        kind: Some(pb::monty_value::Kind::Timedelta(pb::TimeDeltaValue {
            days: 1,
            seconds,
            microseconds,
        })),
    };
    assert!(rejected_as(timedelta(-1, 0), "TimeDeltaValue.seconds"));
    assert!(rejected_as(timedelta(86_400, 0), "TimeDeltaValue.seconds"));
    assert!(rejected_as(timedelta(0, -1), "TimeDeltaValue.microseconds"));
    assert!(rejected_as(timedelta(0, 1_000_000), "TimeDeltaValue.microseconds"));
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
    let proto = pb::MontyError::from(&exc);
    let back = MontyException::try_from(proto).expect("proto -> MontyException failed");
    assert_eq!(back, exc);
    // the rendered traceback (the user-visible artifact) must be identical
    assert_eq!(back.to_string(), exc.to_string());
}

#[test]
fn exception_without_traceback_round_trips() {
    let exc = MontyException::new(ExcType::TypeError, None);
    let back = MontyException::try_from(pb::MontyError::from(&exc)).unwrap();
    assert_eq!(back, exc);
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
    let back = ResourceLimits::try_from(pb::ResourceLimits::from(&limits)).unwrap();
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
    let back = ResourceLimits::try_from(pb::ResourceLimits::default()).unwrap();
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
    for case in &cases {
        let proto = pb::ExtResult::from(case);
        let back = ExtFunctionResult::try_from(proto).unwrap();
        // ExtFunctionResult has no PartialEq; compare via Debug
        assert_eq!(format!("{back:?}"), format!("{case:?}"));
    }
}

#[test]
fn name_lookup_results_convert() {
    let value = pb::ResumeNameLookup {
        kind: Some(pb::resume_name_lookup::Kind::Value(pb::MontyValue::from(
            &MontyObject::Int(1),
        ))),
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
