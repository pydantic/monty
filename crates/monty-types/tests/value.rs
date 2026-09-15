//! Tests for `MontyValue`: Python truthiness, type names, `repr()`,
//! structural equality and the typed accessors.

use monty_types::{
    ExcType, MontyDate, MontyDateTime, MontyGraph, MontyNode, MontyTimeDelta, MontyTimeZone, MontyUuid, MontyValue,
};

/// Tests for `MontyValue::is_truthy()` - Python's truth value testing rules.

#[test]
fn is_truthy_none_is_falsy() {
    assert!(!MontyValue::none().is_truthy());
}

#[test]
fn is_truthy_ellipsis_is_truthy() {
    assert!(MontyValue::ellipsis().is_truthy());
}

#[test]
fn is_truthy_false_is_falsy() {
    assert!(!MontyValue::bool(false).is_truthy());
}

#[test]
fn is_truthy_true_is_truthy() {
    assert!(MontyValue::bool(true).is_truthy());
}

#[test]
fn is_truthy_zero_int_is_falsy() {
    assert!(!MontyValue::int(0).is_truthy());
}

#[test]
fn is_truthy_nonzero_int_is_truthy() {
    assert!(MontyValue::int(1).is_truthy());
    assert!(MontyValue::int(-1).is_truthy());
    assert!(MontyValue::int(42).is_truthy());
}

#[test]
fn is_truthy_zero_float_is_falsy() {
    assert!(!MontyValue::float(0.0).is_truthy());
}

#[test]
fn is_truthy_nonzero_float_is_truthy() {
    assert!(MontyValue::float(1.0).is_truthy());
    assert!(MontyValue::float(-0.5).is_truthy());
    assert!(MontyValue::float(f64::INFINITY).is_truthy());
}

#[test]
fn is_truthy_empty_string_is_falsy() {
    assert!(!MontyValue::string(String::new()).is_truthy());
}

#[test]
fn is_truthy_nonempty_string_is_truthy() {
    assert!(MontyValue::string("hello".to_string()).is_truthy());
    assert!(MontyValue::string(" ".to_string()).is_truthy());
}

#[test]
fn is_truthy_empty_bytes_is_falsy() {
    assert!(!MontyValue::bytes(vec![]).is_truthy());
}

#[test]
fn is_truthy_nonempty_bytes_is_truthy() {
    assert!(MontyValue::bytes(vec![0]).is_truthy());
    assert!(MontyValue::bytes(vec![1, 2, 3]).is_truthy());
}

#[test]
fn is_truthy_empty_list_is_falsy() {
    assert!(!MontyValue::list([]).is_truthy());
}

#[test]
fn is_truthy_nonempty_list_is_truthy() {
    assert!(MontyValue::list([MontyValue::int(1)]).is_truthy());
}

#[test]
fn is_truthy_empty_tuple_is_falsy() {
    assert!(!MontyValue::tuple([]).is_truthy());
}

#[test]
fn is_truthy_nonempty_tuple_is_truthy() {
    assert!(MontyValue::tuple([MontyValue::int(1)]).is_truthy());
}

#[test]
fn is_truthy_empty_dict_is_falsy() {
    assert!(!MontyValue::dict([]).is_truthy());
}

#[test]
fn is_truthy_nonempty_dict_is_truthy() {
    let dict = vec![(MontyValue::string("key".to_string()), MontyValue::int(1))];
    assert!(MontyValue::dict(dict).is_truthy());
}

/// Tests for `MontyValue::type_name()` - Python type names.

#[test]
fn type_name() {
    assert_eq!(MontyValue::none().type_name(), "NoneType");
    assert_eq!(MontyValue::ellipsis().type_name(), "ellipsis");
    assert_eq!(MontyValue::bool(true).type_name(), "bool");
    assert_eq!(MontyValue::bool(false).type_name(), "bool");
    assert_eq!(MontyValue::int(0).type_name(), "int");
    assert_eq!(MontyValue::int(42).type_name(), "int");
    assert_eq!(MontyValue::float(0.0).type_name(), "float");
    assert_eq!(MontyValue::float(2.5).type_name(), "float");
    assert_eq!(MontyValue::string(String::new()).type_name(), "str");
    assert_eq!(MontyValue::string("hello".to_string()).type_name(), "str");
    assert_eq!(MontyValue::bytes(vec![]).type_name(), "bytes");
    assert_eq!(MontyValue::bytes(vec![1, 2, 3]).type_name(), "bytes");
    assert_eq!(MontyValue::list([]).type_name(), "list");
    assert_eq!(MontyValue::tuple([]).type_name(), "tuple");
    assert_eq!(MontyValue::dict([]).type_name(), "dict");
    assert_eq!(MontyValue::set([]).type_name(), "set");
    assert_eq!(MontyValue::frozenset([]).type_name(), "frozenset");
    assert_eq!(
        MontyValue::date(MontyDate {
            year: 2024,
            month: 1,
            day: 1,
        })
        .type_name(),
        "date"
    );
    assert_eq!(
        MontyValue::datetime(MontyDateTime {
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
        MontyValue::timedelta(MontyTimeDelta {
            days: 0,
            seconds: 0,
            microseconds: 0,
        })
        .type_name(),
        "timedelta"
    );
    assert_eq!(
        MontyValue::timezone(MontyTimeZone {
            offset_seconds: 0,
            name: None,
        })
        .type_name(),
        "timezone"
    );
    assert_eq!(
        MontyValue::exception(ExcType::ValueError, None).type_name(),
        "Exception"
    );
    assert_eq!(MontyValue::path("/tmp".to_string()).type_name(), "PosixPath");
    assert_eq!(
        MontyValue::class_instance(
            MontyValue::class_type("Foo", MontyUuid::from_u128(1), false, false, []),
            MontyUuid::from_u128(2),
            [],
        )
        .type_name(),
        "Foo"
    );
}

// === is_truthy for Set, FrozenSet, Date, DateTime, TimeDelta, TimeZone, Exception, Path, Dataclass ===

#[test]
fn is_truthy_set() {
    assert!(!MontyValue::set([]).is_truthy());
    assert!(MontyValue::set([MontyValue::int(1)]).is_truthy());
}

#[test]
fn is_truthy_frozenset() {
    assert!(!MontyValue::frozenset([]).is_truthy());
    assert!(MontyValue::frozenset([MontyValue::int(1)]).is_truthy());
}

#[test]
fn is_truthy_date() {
    assert!(
        MontyValue::date(MontyDate {
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
        MontyValue::datetime(MontyDateTime {
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
        !MontyValue::timedelta(MontyTimeDelta {
            days: 0,
            seconds: 0,
            microseconds: 0,
        })
        .is_truthy()
    );
    // Non-zero timedelta is truthy
    assert!(
        MontyValue::timedelta(MontyTimeDelta {
            days: 1,
            seconds: 0,
            microseconds: 0,
        })
        .is_truthy()
    );
    assert!(
        MontyValue::timedelta(MontyTimeDelta {
            days: 0,
            seconds: 1,
            microseconds: 0,
        })
        .is_truthy()
    );
    assert!(
        MontyValue::timedelta(MontyTimeDelta {
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
        MontyValue::timezone(MontyTimeZone {
            offset_seconds: 0,
            name: None,
        })
        .is_truthy()
    );
}

#[test]
fn is_truthy_exception() {
    assert!(MontyValue::exception(ExcType::ValueError, Some("oops".to_string())).is_truthy());
}

#[test]
fn is_truthy_path() {
    assert!(MontyValue::path("/tmp".to_string()).is_truthy());
}

#[test]
fn is_truthy_class_instance() {
    assert!(
        MontyValue::class_instance(
            MontyValue::class_type("Foo", MontyUuid::from_u128(1), false, false, []),
            MontyUuid::from_u128(2),
            [],
        )
        .is_truthy()
    );
}

// === py_repr tests for datetime types ===

#[test]
fn repr_frozenset_empty() {
    assert_eq!(MontyValue::frozenset([]).py_repr(), "frozenset()");
}

#[test]
fn repr_frozenset_nonempty() {
    let fs = MontyValue::frozenset([MontyValue::int(1), MontyValue::int(2)]);
    assert_eq!(fs.py_repr(), "frozenset({1, 2})");
}

#[test]
fn repr_date() {
    let date = MontyValue::date(MontyDate {
        year: 2024,
        month: 6,
        day: 15,
    });
    assert_eq!(date.py_repr(), "datetime.date(2024, 6, 15)");
}

#[test]
fn repr_datetime_naive() {
    let dt = MontyValue::datetime(MontyDateTime {
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
    let dt = MontyValue::datetime(MontyDateTime {
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
    let dt = MontyValue::datetime(MontyDateTime {
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
    let dt = MontyValue::datetime(MontyDateTime {
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
    let dt = MontyValue::datetime(MontyDateTime {
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
    let td = MontyValue::timedelta(MontyTimeDelta {
        days: 0,
        seconds: 0,
        microseconds: 0,
    });
    assert_eq!(td.py_repr(), "datetime.timedelta(0)");
}

#[test]
fn repr_timedelta_days_only() {
    let td = MontyValue::timedelta(MontyTimeDelta {
        days: 5,
        seconds: 0,
        microseconds: 0,
    });
    assert_eq!(td.py_repr(), "datetime.timedelta(days=5)");
}

#[test]
fn repr_timedelta_seconds_only() {
    let td = MontyValue::timedelta(MontyTimeDelta {
        days: 0,
        seconds: 3600,
        microseconds: 0,
    });
    assert_eq!(td.py_repr(), "datetime.timedelta(seconds=3600)");
}

#[test]
fn repr_timedelta_microseconds_only() {
    let td = MontyValue::timedelta(MontyTimeDelta {
        days: 0,
        seconds: 0,
        microseconds: 500,
    });
    assert_eq!(td.py_repr(), "datetime.timedelta(microseconds=500)");
}

#[test]
fn repr_timedelta_all_components() {
    let td = MontyValue::timedelta(MontyTimeDelta {
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
    let tz = MontyValue::timezone(MontyTimeZone {
        offset_seconds: 0,
        name: None,
    });
    assert_eq!(tz.py_repr(), "datetime.timezone.utc");
}

#[test]
fn repr_timezone_with_offset() {
    let tz = MontyValue::timezone(MontyTimeZone {
        offset_seconds: 3600,
        name: None,
    });
    assert_eq!(tz.py_repr(), "datetime.timezone(datetime.timedelta(seconds=3600))");
}

#[test]
fn repr_timezone_with_name() {
    let tz = MontyValue::timezone(MontyTimeZone {
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
    let exc = MontyValue::exception(ExcType::ValueError, None);
    assert_eq!(exc.py_repr(), "ValueError()");
}

#[test]
fn repr_exception_with_arg() {
    let exc = MontyValue::exception(ExcType::TypeError, Some("bad type".to_string()));
    assert_eq!(exc.py_repr(), "TypeError('bad type')");
}

// === PartialEq ===

#[test]
fn eq_date() {
    let a = MontyValue::date(MontyDate {
        year: 2024,
        month: 6,
        day: 15,
    });
    let b = a.clone();
    assert_eq!(a, b);
}

#[test]
fn eq_datetime() {
    let a = MontyValue::datetime(MontyDateTime {
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
    let utc = MontyValue::datetime(MontyDateTime {
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
    let plus_one = MontyValue::datetime(MontyDateTime {
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
    let a = MontyValue::timedelta(MontyTimeDelta {
        days: 5,
        seconds: 100,
        microseconds: 999,
    });
    let b = a.clone();
    assert_eq!(a, b);
}

#[test]
fn eq_timezone() {
    let a = MontyValue::timezone(MontyTimeZone {
        offset_seconds: -3600,
        name: Some("EST".to_string()),
    });
    let b = MontyValue::timezone(MontyTimeZone {
        offset_seconds: -3600,
        name: Some("UTC-1".to_string()),
    });
    assert_eq!(a, b);
}

#[test]
fn eq_named_tuple() {
    let a = MontyValue::named_tuple(
        "Point".to_string(),
        vec!["x".to_string(), "y".to_string()],
        vec![MontyValue::int(1), MontyValue::int(2)],
    );
    let b = a.clone();
    assert_eq!(a, b);
}

#[test]
fn eq_int_and_bigint() {
    assert_eq!(MontyValue::int(7), MontyValue::bigint(7.into()));
    assert_ne!(
        MontyValue::int(7),
        MontyValue::bigint("123456789012345678901234567890".parse().unwrap())
    );
}

#[test]
fn eq_float_by_bits() {
    assert_eq!(MontyValue::float(f64::NAN), MontyValue::float(f64::NAN));
    assert_ne!(MontyValue::float(0.0), MontyValue::float(-0.0));
}

#[test]
fn eq_named_tuple_with_tuple() {
    let named = MontyValue::named_tuple("Point", ["x", "y"], [MontyValue::int(1), MontyValue::int(2)]);
    assert_eq!(named, MontyValue::tuple([MontyValue::int(1), MontyValue::int(2)]));
    assert_ne!(named, MontyValue::list([MontyValue::int(1), MontyValue::int(2)]));
}

#[test]
fn eq_ignores_arena_layout() {
    // one value with sharing, one with copies, one with an unreachable node
    let mut graph = MontyGraph::new();
    let one = graph.push(MontyNode::Int(1));
    let inner = graph.push(MontyNode::List(vec![one]));
    let root = graph.push(MontyNode::List(vec![inner, inner]));
    let shared = MontyValue::new(graph, root).unwrap();
    let copied = MontyValue::list([
        MontyValue::list([MontyValue::int(1)]),
        MontyValue::list([MontyValue::int(1)]),
    ]);
    let mut padded = copied.clone();
    padded.graph.merge(MontyValue::string("unreachable").graph);
    assert_ne!(shared.graph.len(), copied.graph.len());
    assert_eq!(shared, copied);
    assert_eq!(copied, padded);
    assert_ne!(
        shared,
        MontyValue::list([MontyValue::list([MontyValue::int(1)]), MontyValue::list([])])
    );
}

#[test]
fn eq_class_instances() {
    let class = || MontyValue::class_type("Foo", MontyUuid::from_u128(1), true, false, []);
    let a = MontyValue::class_instance(
        class(),
        MontyUuid::from_u128(2),
        [(MontyValue::string("x"), MontyValue::int(1))],
    );
    assert_eq!(a, a.clone());
    let other_id = MontyValue::class_instance(
        class(),
        MontyUuid::from_u128(3),
        [(MontyValue::string("x"), MontyValue::int(1))],
    );
    assert_ne!(a, other_id);
    let other_attrs = MontyValue::class_instance(class(), MontyUuid::from_u128(2), []);
    assert_ne!(a, other_attrs);
}

// === accessors and Display ===

#[test]
fn accessors_read_leaves_and_containers() {
    let value = MontyValue::dict([(
        MontyValue::string("k"),
        MontyValue::list([MontyValue::int(1), MontyValue::bool(true)]),
    )]);
    let (key, items) = value.as_ref().pairs().unwrap().into_iter().next().unwrap();
    assert_eq!(key.as_str(), Some("k"));
    let items = items.items().unwrap();
    assert_eq!(items[0].as_int(), Some(1));
    assert_eq!(items[0].as_float(), Some(1.0));
    assert_eq!(items[1].as_bool(), Some(true));
    assert_eq!(items[1].as_int(), None);
    assert!(value.as_ref().items().is_none());
    assert_eq!(i64::try_from(items[0]).unwrap(), 1);
    assert_eq!(
        String::try_from(items[0]).unwrap_err().to_string(),
        "expected str, got int"
    );
}

#[test]
fn display_is_str_and_py_repr_is_repr() {
    let text = MontyValue::string("hi");
    assert_eq!(text.to_string(), "hi");
    assert_eq!(text.py_repr(), "'hi'");
    let list = MontyValue::list([text, MontyValue::none(), MontyValue::cycle("[...]")]);
    assert_eq!(list.to_string(), "['hi', None, [...]]");
    assert_eq!(
        MontyValue::class_type("Foo", MontyUuid::from_u128(1), true, false, []).to_string(),
        "<class 'Foo'>"
    );
    let instance = MontyValue::class_instance(
        MontyValue::class_type("Foo", MontyUuid::from_u128(1), true, false, []),
        MontyUuid::from_u128(2),
        [(MontyValue::string("x"), MontyValue::int(1))],
    );
    assert_eq!(instance.py_repr(), "Foo(x=1)");
    assert_eq!(instance.type_name(), "Foo");
}
