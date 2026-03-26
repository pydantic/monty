use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use monty::{ExcType, MontyDate, MontyDateTime, MontyObject, MontyTimeDelta, MontyTimeZone};

/// Helper to compute a hash for a value.
fn hash_of(obj: &MontyObject) -> u64 {
    let mut hasher = DefaultHasher::new();
    obj.hash(&mut hasher);
    hasher.finish()
}

/// Tests for `MontyObject::is_truthy()` - Python's truth value testing rules.

#[test]
fn is_truthy_none_is_falsy() {
    assert!(!MontyObject::None.is_truthy());
}

#[test]
fn is_truthy_ellipsis_is_truthy() {
    assert!(MontyObject::Ellipsis.is_truthy());
}

#[test]
fn is_truthy_false_is_falsy() {
    assert!(!MontyObject::Bool(false).is_truthy());
}

#[test]
fn is_truthy_true_is_truthy() {
    assert!(MontyObject::Bool(true).is_truthy());
}

#[test]
fn is_truthy_zero_int_is_falsy() {
    assert!(!MontyObject::Int(0).is_truthy());
}

#[test]
fn is_truthy_nonzero_int_is_truthy() {
    assert!(MontyObject::Int(1).is_truthy());
    assert!(MontyObject::Int(-1).is_truthy());
    assert!(MontyObject::Int(42).is_truthy());
}

#[test]
fn is_truthy_zero_float_is_falsy() {
    assert!(!MontyObject::Float(0.0).is_truthy());
}

#[test]
fn is_truthy_nonzero_float_is_truthy() {
    assert!(MontyObject::Float(1.0).is_truthy());
    assert!(MontyObject::Float(-0.5).is_truthy());
    assert!(MontyObject::Float(f64::INFINITY).is_truthy());
}

#[test]
fn is_truthy_empty_string_is_falsy() {
    assert!(!MontyObject::String(String::new()).is_truthy());
}

#[test]
fn is_truthy_nonempty_string_is_truthy() {
    assert!(MontyObject::String("hello".to_string()).is_truthy());
    assert!(MontyObject::String(" ".to_string()).is_truthy());
}

#[test]
fn is_truthy_empty_bytes_is_falsy() {
    assert!(!MontyObject::Bytes(vec![]).is_truthy());
}

#[test]
fn is_truthy_nonempty_bytes_is_truthy() {
    assert!(MontyObject::Bytes(vec![0]).is_truthy());
    assert!(MontyObject::Bytes(vec![1, 2, 3]).is_truthy());
}

#[test]
fn is_truthy_empty_list_is_falsy() {
    assert!(!MontyObject::List(vec![]).is_truthy());
}

#[test]
fn is_truthy_nonempty_list_is_truthy() {
    assert!(MontyObject::List(vec![MontyObject::Int(1)]).is_truthy());
}

#[test]
fn is_truthy_empty_tuple_is_falsy() {
    assert!(!MontyObject::Tuple(vec![]).is_truthy());
}

#[test]
fn is_truthy_nonempty_tuple_is_truthy() {
    assert!(MontyObject::Tuple(vec![MontyObject::Int(1)]).is_truthy());
}

#[test]
fn is_truthy_empty_dict_is_falsy() {
    assert!(!MontyObject::dict(vec![]).is_truthy());
}

#[test]
fn is_truthy_nonempty_dict_is_truthy() {
    let dict = vec![(MontyObject::String("key".to_string()), MontyObject::Int(1))];
    assert!(MontyObject::dict(dict).is_truthy());
}

/// Tests for `MontyObject::type_name()` - Python type names.

#[test]
fn type_name() {
    assert_eq!(MontyObject::None.type_name(), "NoneType");
    assert_eq!(MontyObject::Ellipsis.type_name(), "ellipsis");
    assert_eq!(MontyObject::Bool(true).type_name(), "bool");
    assert_eq!(MontyObject::Bool(false).type_name(), "bool");
    assert_eq!(MontyObject::Int(0).type_name(), "int");
    assert_eq!(MontyObject::Int(42).type_name(), "int");
    assert_eq!(MontyObject::Float(0.0).type_name(), "float");
    assert_eq!(MontyObject::Float(2.5).type_name(), "float");
    assert_eq!(MontyObject::String(String::new()).type_name(), "str");
    assert_eq!(MontyObject::String("hello".to_string()).type_name(), "str");
    assert_eq!(MontyObject::Bytes(vec![]).type_name(), "bytes");
    assert_eq!(MontyObject::Bytes(vec![1, 2, 3]).type_name(), "bytes");
    assert_eq!(MontyObject::List(vec![]).type_name(), "list");
    assert_eq!(MontyObject::Tuple(vec![]).type_name(), "tuple");
    assert_eq!(MontyObject::dict(vec![]).type_name(), "dict");
    assert_eq!(MontyObject::Set(vec![]).type_name(), "set");
    assert_eq!(MontyObject::FrozenSet(vec![]).type_name(), "frozenset");
    assert_eq!(
        MontyObject::Date(MontyDate {
            year: 2024,
            month: 1,
            day: 1,
        })
        .type_name(),
        "date"
    );
    assert_eq!(
        MontyObject::DateTime(MontyDateTime {
            year: 2024,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            offset_seconds: None,
            timezone_name: None,
        })
        .type_name(),
        "datetime"
    );
    assert_eq!(
        MontyObject::TimeDelta(MontyTimeDelta {
            days: 0,
            seconds: 0,
            microseconds: 0,
        })
        .type_name(),
        "timedelta"
    );
    assert_eq!(
        MontyObject::TimeZone(MontyTimeZone {
            offset_seconds: 0,
            name: None,
        })
        .type_name(),
        "timezone"
    );
    assert_eq!(
        MontyObject::Exception {
            exc_type: ExcType::ValueError,
            arg: None,
        }
        .type_name(),
        "Exception"
    );
    assert_eq!(MontyObject::Path("/tmp".to_string()).type_name(), "PosixPath");
    assert_eq!(
        MontyObject::Dataclass {
            name: "Foo".to_string(),
            type_id: 0,
            field_names: vec![],
            attrs: monty::DictPairs::from(vec![]),
            frozen: false,
        }
        .type_name(),
        "dataclass"
    );
}

// === is_truthy for Set, FrozenSet, Date, DateTime, TimeDelta, TimeZone, Exception, Path, Dataclass ===

#[test]
fn is_truthy_set() {
    assert!(!MontyObject::Set(vec![]).is_truthy());
    assert!(MontyObject::Set(vec![MontyObject::Int(1)]).is_truthy());
}

#[test]
fn is_truthy_frozenset() {
    assert!(!MontyObject::FrozenSet(vec![]).is_truthy());
    assert!(MontyObject::FrozenSet(vec![MontyObject::Int(1)]).is_truthy());
}

#[test]
fn is_truthy_date() {
    assert!(
        MontyObject::Date(MontyDate {
            year: 2024,
            month: 6,
            day: 15,
        })
        .is_truthy()
    );
}

#[test]
fn is_truthy_datetime() {
    assert!(
        MontyObject::DateTime(MontyDateTime {
            year: 2024,
            month: 6,
            day: 15,
            hour: 12,
            minute: 30,
            second: 0,
            microsecond: 0,
            offset_seconds: None,
            timezone_name: None,
        })
        .is_truthy()
    );
}

#[test]
fn is_truthy_timedelta() {
    // Zero timedelta is falsy
    assert!(
        !MontyObject::TimeDelta(MontyTimeDelta {
            days: 0,
            seconds: 0,
            microseconds: 0,
        })
        .is_truthy()
    );
    // Non-zero timedelta is truthy
    assert!(
        MontyObject::TimeDelta(MontyTimeDelta {
            days: 1,
            seconds: 0,
            microseconds: 0,
        })
        .is_truthy()
    );
    assert!(
        MontyObject::TimeDelta(MontyTimeDelta {
            days: 0,
            seconds: 1,
            microseconds: 0,
        })
        .is_truthy()
    );
    assert!(
        MontyObject::TimeDelta(MontyTimeDelta {
            days: 0,
            seconds: 0,
            microseconds: 1,
        })
        .is_truthy()
    );
}

#[test]
fn is_truthy_timezone() {
    assert!(
        MontyObject::TimeZone(MontyTimeZone {
            offset_seconds: 0,
            name: None,
        })
        .is_truthy()
    );
}

#[test]
fn is_truthy_exception() {
    assert!(
        MontyObject::Exception {
            exc_type: ExcType::ValueError,
            arg: Some("oops".to_string()),
        }
        .is_truthy()
    );
}

#[test]
fn is_truthy_path() {
    assert!(MontyObject::Path("/tmp".to_string()).is_truthy());
}

#[test]
fn is_truthy_dataclass() {
    assert!(
        MontyObject::Dataclass {
            name: "Foo".to_string(),
            type_id: 0,
            field_names: vec![],
            attrs: monty::DictPairs::from(vec![]),
            frozen: false,
        }
        .is_truthy()
    );
}

// === py_repr tests for datetime types ===

#[test]
fn repr_frozenset_empty() {
    assert_eq!(MontyObject::FrozenSet(vec![]).py_repr(), "frozenset()");
}

#[test]
fn repr_frozenset_nonempty() {
    let fs = MontyObject::FrozenSet(vec![MontyObject::Int(1), MontyObject::Int(2)]);
    assert_eq!(fs.py_repr(), "frozenset({1, 2})");
}

#[test]
fn repr_date() {
    let date = MontyObject::Date(MontyDate {
        year: 2024,
        month: 6,
        day: 15,
    });
    assert_eq!(date.py_repr(), "datetime.date(2024, 6, 15)");
}

#[test]
fn repr_datetime_naive() {
    let dt = MontyObject::DateTime(MontyDateTime {
        year: 2024,
        month: 6,
        day: 15,
        hour: 12,
        minute: 30,
        second: 0,
        microsecond: 0,
        offset_seconds: None,
        timezone_name: None,
    });
    assert_eq!(dt.py_repr(), "datetime.datetime(2024, 6, 15, 12, 30)");
}

#[test]
fn repr_datetime_with_seconds_and_microseconds() {
    let dt = MontyObject::DateTime(MontyDateTime {
        year: 2024,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 45,
        microsecond: 123_456,
        offset_seconds: None,
        timezone_name: None,
    });
    assert_eq!(dt.py_repr(), "datetime.datetime(2024, 1, 1, 0, 0, 45, 123456)");
}

#[test]
fn repr_datetime_utc() {
    let dt = MontyObject::DateTime(MontyDateTime {
        year: 2024,
        month: 6,
        day: 15,
        hour: 12,
        minute: 0,
        second: 0,
        microsecond: 0,
        offset_seconds: Some(0),
        timezone_name: None,
    });
    assert_eq!(
        dt.py_repr(),
        "datetime.datetime(2024, 6, 15, 12, 0, tzinfo=datetime.timezone.utc)"
    );
}

#[test]
fn repr_datetime_with_offset() {
    let dt = MontyObject::DateTime(MontyDateTime {
        year: 2024,
        month: 6,
        day: 15,
        hour: 12,
        minute: 0,
        second: 0,
        microsecond: 0,
        offset_seconds: Some(3600),
        timezone_name: None,
    });
    assert_eq!(
        dt.py_repr(),
        "datetime.datetime(2024, 6, 15, 12, 0, tzinfo=datetime.timezone(datetime.timedelta(seconds=3600)))"
    );
}

#[test]
fn repr_datetime_with_named_timezone() {
    let dt = MontyObject::DateTime(MontyDateTime {
        year: 2024,
        month: 6,
        day: 15,
        hour: 12,
        minute: 0,
        second: 0,
        microsecond: 0,
        offset_seconds: Some(3600),
        timezone_name: Some("CET".to_string()),
    });
    assert_eq!(
        dt.py_repr(),
        "datetime.datetime(2024, 6, 15, 12, 0, tzinfo=datetime.timezone(datetime.timedelta(seconds=3600), 'CET'))"
    );
}

#[test]
fn repr_timedelta_zero() {
    let td = MontyObject::TimeDelta(MontyTimeDelta {
        days: 0,
        seconds: 0,
        microseconds: 0,
    });
    assert_eq!(td.py_repr(), "datetime.timedelta(0)");
}

#[test]
fn repr_timedelta_days_only() {
    let td = MontyObject::TimeDelta(MontyTimeDelta {
        days: 5,
        seconds: 0,
        microseconds: 0,
    });
    assert_eq!(td.py_repr(), "datetime.timedelta(days=5)");
}

#[test]
fn repr_timedelta_seconds_only() {
    let td = MontyObject::TimeDelta(MontyTimeDelta {
        days: 0,
        seconds: 3600,
        microseconds: 0,
    });
    assert_eq!(td.py_repr(), "datetime.timedelta(seconds=3600)");
}

#[test]
fn repr_timedelta_microseconds_only() {
    let td = MontyObject::TimeDelta(MontyTimeDelta {
        days: 0,
        seconds: 0,
        microseconds: 500,
    });
    assert_eq!(td.py_repr(), "datetime.timedelta(microseconds=500)");
}

#[test]
fn repr_timedelta_all_components() {
    let td = MontyObject::TimeDelta(MontyTimeDelta {
        days: 1,
        seconds: 3600,
        microseconds: 500,
    });
    assert_eq!(
        td.py_repr(),
        "datetime.timedelta(days=1, seconds=3600, microseconds=500)"
    );
}

#[test]
fn repr_timezone_utc() {
    let tz = MontyObject::TimeZone(MontyTimeZone {
        offset_seconds: 0,
        name: None,
    });
    assert_eq!(tz.py_repr(), "datetime.timezone.utc");
}

#[test]
fn repr_timezone_with_offset() {
    let tz = MontyObject::TimeZone(MontyTimeZone {
        offset_seconds: 3600,
        name: None,
    });
    assert_eq!(tz.py_repr(), "datetime.timezone(datetime.timedelta(seconds=3600))");
}

#[test]
fn repr_timezone_with_name() {
    let tz = MontyObject::TimeZone(MontyTimeZone {
        offset_seconds: 3600,
        name: Some("CET".to_string()),
    });
    assert_eq!(
        tz.py_repr(),
        "datetime.timezone(datetime.timedelta(seconds=3600), 'CET')"
    );
}

#[test]
fn repr_exception_no_arg() {
    let exc = MontyObject::Exception {
        exc_type: ExcType::ValueError,
        arg: None,
    };
    assert_eq!(exc.py_repr(), "ValueError()");
}

#[test]
fn repr_exception_with_arg() {
    let exc = MontyObject::Exception {
        exc_type: ExcType::TypeError,
        arg: Some("bad type".to_string()),
    };
    assert_eq!(exc.py_repr(), "TypeError('bad type')");
}

// === Hash and PartialEq tests ===

#[test]
fn hash_float() {
    let a = MontyObject::Float(1.5);
    let b = MontyObject::Float(1.5);
    assert_eq!(hash_of(&a), hash_of(&b));
}

#[test]
fn hash_bytes() {
    let a = MontyObject::Bytes(vec![1, 2, 3]);
    let b = MontyObject::Bytes(vec![1, 2, 3]);
    assert_eq!(hash_of(&a), hash_of(&b));
}

#[test]
fn hash_date() {
    let a = MontyObject::Date(MontyDate {
        year: 2024,
        month: 6,
        day: 15,
    });
    let b = MontyObject::Date(MontyDate {
        year: 2024,
        month: 6,
        day: 15,
    });
    assert_eq!(hash_of(&a), hash_of(&b));
}

#[test]
fn hash_datetime() {
    let a = MontyObject::DateTime(MontyDateTime {
        year: 2024,
        month: 1,
        day: 1,
        hour: 12,
        minute: 0,
        second: 0,
        microsecond: 0,
        offset_seconds: None,
        timezone_name: None,
    });
    let b = a.clone();
    assert_eq!(hash_of(&a), hash_of(&b));
}

#[test]
fn hash_datetime_aware_uses_utc_instant() {
    let utc = MontyObject::DateTime(MontyDateTime {
        year: 2024,
        month: 1,
        day: 1,
        hour: 12,
        minute: 0,
        second: 0,
        microsecond: 0,
        offset_seconds: Some(0),
        timezone_name: None,
    });
    let plus_one = MontyObject::DateTime(MontyDateTime {
        year: 2024,
        month: 1,
        day: 1,
        hour: 13,
        minute: 0,
        second: 0,
        microsecond: 0,
        offset_seconds: Some(3600),
        timezone_name: Some("PLUS1".to_string()),
    });
    assert_eq!(hash_of(&utc), hash_of(&plus_one));
}

#[test]
fn hash_timedelta() {
    let a = MontyObject::TimeDelta(MontyTimeDelta {
        days: 1,
        seconds: 3600,
        microseconds: 0,
    });
    let b = a.clone();
    assert_eq!(hash_of(&a), hash_of(&b));
}

#[test]
fn hash_timezone() {
    let a = MontyObject::TimeZone(MontyTimeZone {
        offset_seconds: 3600,
        name: Some("CET".to_string()),
    });
    let b = MontyObject::TimeZone(MontyTimeZone {
        offset_seconds: 3600,
        name: Some("BST".to_string()),
    });
    assert_eq!(hash_of(&a), hash_of(&b));
}

#[test]
fn hash_path() {
    let a = MontyObject::Path("/tmp/foo".to_string());
    let b = MontyObject::Path("/tmp/foo".to_string());
    assert_eq!(hash_of(&a), hash_of(&b));
}

#[test]
fn eq_date() {
    let a = MontyObject::Date(MontyDate {
        year: 2024,
        month: 6,
        day: 15,
    });
    let b = a.clone();
    assert_eq!(a, b);
}

#[test]
fn eq_datetime() {
    let a = MontyObject::DateTime(MontyDateTime {
        year: 2024,
        month: 1,
        day: 1,
        hour: 12,
        minute: 0,
        second: 0,
        microsecond: 0,
        offset_seconds: Some(0),
        timezone_name: None,
    });
    let b = a.clone();
    assert_eq!(a, b);
}

#[test]
fn eq_datetime_aware_uses_utc_instant() {
    let utc = MontyObject::DateTime(MontyDateTime {
        year: 2024,
        month: 1,
        day: 1,
        hour: 12,
        minute: 0,
        second: 0,
        microsecond: 0,
        offset_seconds: Some(0),
        timezone_name: None,
    });
    let plus_one = MontyObject::DateTime(MontyDateTime {
        year: 2024,
        month: 1,
        day: 1,
        hour: 13,
        minute: 0,
        second: 0,
        microsecond: 0,
        offset_seconds: Some(3600),
        timezone_name: Some("PLUS1".to_string()),
    });
    assert_eq!(utc, plus_one);
}

#[test]
fn eq_timedelta() {
    let a = MontyObject::TimeDelta(MontyTimeDelta {
        days: 5,
        seconds: 100,
        microseconds: 999,
    });
    let b = a.clone();
    assert_eq!(a, b);
}

#[test]
fn eq_timezone() {
    let a = MontyObject::TimeZone(MontyTimeZone {
        offset_seconds: -3600,
        name: Some("EST".to_string()),
    });
    let b = MontyObject::TimeZone(MontyTimeZone {
        offset_seconds: -3600,
        name: Some("UTC-1".to_string()),
    });
    assert_eq!(a, b);
}

#[test]
fn eq_named_tuple() {
    let a = MontyObject::NamedTuple {
        type_name: "Point".to_string(),
        field_names: vec!["x".to_string(), "y".to_string()],
        values: vec![MontyObject::Int(1), MontyObject::Int(2)],
    };
    let b = a.clone();
    assert_eq!(a, b);
}
