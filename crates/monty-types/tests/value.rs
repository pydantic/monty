//! Tests for `MontyObject`: Python truthiness, type names, `repr()`,
//! structural equality and the typed accessors.

use monty_types::{
    ExcType, MontyDate, MontyDateTime, MontyGraph, MontyNode, MontyObject, MontyTimeDelta, MontyTimeZone, MontyUuid,
};

// === is_truthy ===

#[test]
fn is_truthy_none_is_falsy() {
    assert!(!MontyObject::none().is_truthy());
}

#[test]
fn is_truthy_ellipsis_is_truthy() {
    assert!(MontyObject::ellipsis().is_truthy());
}

#[test]
fn is_truthy_false_is_falsy() {
    assert!(!MontyObject::bool(false).is_truthy());
}

#[test]
fn is_truthy_true_is_truthy() {
    assert!(MontyObject::bool(true).is_truthy());
}

#[test]
fn is_truthy_zero_int_is_falsy() {
    assert!(!MontyObject::int(0).is_truthy());
}

#[test]
fn is_truthy_nonzero_int_is_truthy() {
    assert!(MontyObject::int(1).is_truthy());
    assert!(MontyObject::int(-1).is_truthy());
    assert!(MontyObject::int(42).is_truthy());
}

#[test]
fn is_truthy_zero_float_is_falsy() {
    assert!(!MontyObject::float(0.0).is_truthy());
}

#[test]
fn is_truthy_nonzero_float_is_truthy() {
    assert!(MontyObject::float(1.0).is_truthy());
    assert!(MontyObject::float(-0.5).is_truthy());
    assert!(MontyObject::float(f64::INFINITY).is_truthy());
}

#[test]
fn is_truthy_empty_string_is_falsy() {
    assert!(!MontyObject::string(String::new()).is_truthy());
}

#[test]
fn is_truthy_nonempty_string_is_truthy() {
    assert!(MontyObject::string("hello".to_string()).is_truthy());
    assert!(MontyObject::string(" ".to_string()).is_truthy());
}

#[test]
fn is_truthy_empty_bytes_is_falsy() {
    assert!(!MontyObject::bytes(vec![]).is_truthy());
}

#[test]
fn is_truthy_nonempty_bytes_is_truthy() {
    assert!(MontyObject::bytes(vec![0]).is_truthy());
    assert!(MontyObject::bytes(vec![1, 2, 3]).is_truthy());
}

#[test]
fn is_truthy_empty_list_is_falsy() {
    assert!(!MontyObject::list([]).is_truthy());
}

#[test]
fn is_truthy_nonempty_list_is_truthy() {
    assert!(MontyObject::list([MontyObject::int(1)]).is_truthy());
}

#[test]
fn is_truthy_empty_tuple_is_falsy() {
    assert!(!MontyObject::tuple([]).is_truthy());
}

#[test]
fn is_truthy_nonempty_tuple_is_truthy() {
    assert!(MontyObject::tuple([MontyObject::int(1)]).is_truthy());
}

#[test]
fn is_truthy_empty_dict_is_falsy() {
    assert!(!MontyObject::dict([]).is_truthy());
}

#[test]
fn is_truthy_nonempty_dict_is_truthy() {
    let dict = vec![(MontyObject::string("key".to_string()), MontyObject::int(1))];
    assert!(MontyObject::dict(dict).is_truthy());
}

// === type_name ===

#[test]
fn type_name() {
    assert_eq!(MontyObject::none().type_name(), "NoneType");
    assert_eq!(MontyObject::ellipsis().type_name(), "ellipsis");
    assert_eq!(MontyObject::bool(true).type_name(), "bool");
    assert_eq!(MontyObject::bool(false).type_name(), "bool");
    assert_eq!(MontyObject::int(0).type_name(), "int");
    assert_eq!(MontyObject::int(42).type_name(), "int");
    assert_eq!(MontyObject::float(0.0).type_name(), "float");
    assert_eq!(MontyObject::float(2.5).type_name(), "float");
    assert_eq!(MontyObject::string(String::new()).type_name(), "str");
    assert_eq!(MontyObject::string("hello".to_string()).type_name(), "str");
    assert_eq!(MontyObject::bytes(vec![]).type_name(), "bytes");
    assert_eq!(MontyObject::bytes(vec![1, 2, 3]).type_name(), "bytes");
    assert_eq!(MontyObject::list([]).type_name(), "list");
    assert_eq!(MontyObject::tuple([]).type_name(), "tuple");
    assert_eq!(MontyObject::dict([]).type_name(), "dict");
    assert_eq!(MontyObject::set([]).type_name(), "set");
    assert_eq!(MontyObject::frozenset([]).type_name(), "frozenset");
    assert_eq!(
        MontyObject::date(MontyDate {
            year: 2024,
            month: 1,
            day: 1,
        })
        .type_name(),
        "date"
    );
    assert_eq!(
        MontyObject::datetime(MontyDateTime {
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
        MontyObject::timedelta(MontyTimeDelta {
            days: 0,
            seconds: 0,
            microseconds: 0,
        })
        .type_name(),
        "timedelta"
    );
    assert_eq!(
        MontyObject::timezone(MontyTimeZone {
            offset_seconds: 0,
            name: None,
        })
        .type_name(),
        "timezone"
    );
    assert_eq!(
        MontyObject::exception(ExcType::ValueError, None).type_name(),
        "Exception"
    );
    assert_eq!(MontyObject::path("/tmp".to_string()).type_name(), "PosixPath");
    assert_eq!(
        MontyObject::class_instance(
            MontyObject::class_type("Foo", MontyUuid::from_u128(1), false, false, []),
            MontyUuid::from_u128(2),
            [],
        )
        .type_name(),
        "Foo"
    );
}

// === is_truthy for the remaining kinds ===

#[test]
fn is_truthy_set() {
    assert!(!MontyObject::set([]).is_truthy());
    assert!(MontyObject::set([MontyObject::int(1)]).is_truthy());
}

#[test]
fn is_truthy_frozenset() {
    assert!(!MontyObject::frozenset([]).is_truthy());
    assert!(MontyObject::frozenset([MontyObject::int(1)]).is_truthy());
}

#[test]
fn is_truthy_date() {
    assert!(
        MontyObject::date(MontyDate {
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
        MontyObject::datetime(MontyDateTime {
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
    assert!(
        !MontyObject::timedelta(MontyTimeDelta {
            days: 0,
            seconds: 0,
            microseconds: 0,
        })
        .is_truthy()
    );
    assert!(
        MontyObject::timedelta(MontyTimeDelta {
            days: 1,
            seconds: 0,
            microseconds: 0,
        })
        .is_truthy()
    );
    assert!(
        MontyObject::timedelta(MontyTimeDelta {
            days: 0,
            seconds: 1,
            microseconds: 0,
        })
        .is_truthy()
    );
    assert!(
        MontyObject::timedelta(MontyTimeDelta {
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
        MontyObject::timezone(MontyTimeZone {
            offset_seconds: 0,
            name: None,
        })
        .is_truthy()
    );
}

#[test]
fn is_truthy_exception() {
    assert!(MontyObject::exception(ExcType::ValueError, Some("oops".to_string())).is_truthy());
}

#[test]
fn is_truthy_path() {
    assert!(MontyObject::path("/tmp".to_string()).is_truthy());
}

#[test]
fn is_truthy_class_instance() {
    assert!(
        MontyObject::class_instance(
            MontyObject::class_type("Foo", MontyUuid::from_u128(1), false, false, []),
            MontyUuid::from_u128(2),
            [],
        )
        .is_truthy()
    );
}

// === py_repr tests for datetime types ===

#[test]
fn repr_frozenset_empty() {
    assert_eq!(MontyObject::frozenset([]).py_repr(), "frozenset()");
}

#[test]
fn repr_frozenset_nonempty() {
    let fs = MontyObject::frozenset([MontyObject::int(1), MontyObject::int(2)]);
    assert_eq!(fs.py_repr(), "frozenset({1, 2})");
}

#[test]
fn repr_date() {
    let date = MontyObject::date(MontyDate {
        year: 2024,
        month: 6,
        day: 15,
    });
    assert_eq!(date.py_repr(), "datetime.date(2024, 6, 15)");
}

#[test]
fn repr_datetime_naive() {
    let dt = MontyObject::datetime(MontyDateTime {
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
    let dt = MontyObject::datetime(MontyDateTime {
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
    let dt = MontyObject::datetime(MontyDateTime {
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
    let dt = MontyObject::datetime(MontyDateTime {
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
    let dt = MontyObject::datetime(MontyDateTime {
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
    let td = MontyObject::timedelta(MontyTimeDelta {
        days: 0,
        seconds: 0,
        microseconds: 0,
    });
    assert_eq!(td.py_repr(), "datetime.timedelta(0)");
}

#[test]
fn repr_timedelta_days_only() {
    let td = MontyObject::timedelta(MontyTimeDelta {
        days: 5,
        seconds: 0,
        microseconds: 0,
    });
    assert_eq!(td.py_repr(), "datetime.timedelta(days=5)");
}

#[test]
fn repr_timedelta_seconds_only() {
    let td = MontyObject::timedelta(MontyTimeDelta {
        days: 0,
        seconds: 3600,
        microseconds: 0,
    });
    assert_eq!(td.py_repr(), "datetime.timedelta(seconds=3600)");
}

#[test]
fn repr_timedelta_microseconds_only() {
    let td = MontyObject::timedelta(MontyTimeDelta {
        days: 0,
        seconds: 0,
        microseconds: 500,
    });
    assert_eq!(td.py_repr(), "datetime.timedelta(microseconds=500)");
}

#[test]
fn repr_timedelta_all_components() {
    let td = MontyObject::timedelta(MontyTimeDelta {
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
    let tz = MontyObject::timezone(MontyTimeZone {
        offset_seconds: 0,
        name: None,
    });
    assert_eq!(tz.py_repr(), "datetime.timezone.utc");
}

#[test]
fn repr_timezone_with_offset() {
    let tz = MontyObject::timezone(MontyTimeZone {
        offset_seconds: 3600,
        name: None,
    });
    assert_eq!(tz.py_repr(), "datetime.timezone(datetime.timedelta(seconds=3600))");
}

#[test]
fn repr_timezone_with_name() {
    let tz = MontyObject::timezone(MontyTimeZone {
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
    let exc = MontyObject::exception(ExcType::ValueError, None);
    assert_eq!(exc.py_repr(), "ValueError()");
}

#[test]
fn repr_exception_with_arg() {
    let exc = MontyObject::exception(ExcType::TypeError, Some("bad type".to_string()));
    assert_eq!(exc.py_repr(), "TypeError('bad type')");
}

// === PartialEq ===

#[test]
fn eq_date() {
    let a = MontyObject::date(MontyDate {
        year: 2024,
        month: 6,
        day: 15,
    });
    let b = a.clone();
    assert_eq!(a, b);
}

#[test]
fn eq_datetime() {
    let a = MontyObject::datetime(MontyDateTime {
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
    let utc = MontyObject::datetime(MontyDateTime {
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
    let plus_one = MontyObject::datetime(MontyDateTime {
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
    let a = MontyObject::timedelta(MontyTimeDelta {
        days: 5,
        seconds: 100,
        microseconds: 999,
    });
    let b = a.clone();
    assert_eq!(a, b);
}

#[test]
fn eq_timezone() {
    let a = MontyObject::timezone(MontyTimeZone {
        offset_seconds: -3600,
        name: Some("EST".to_string()),
    });
    let b = MontyObject::timezone(MontyTimeZone {
        offset_seconds: -3600,
        name: Some("UTC-1".to_string()),
    });
    assert_eq!(a, b);
}

#[test]
fn eq_named_tuple() {
    let a = MontyObject::named_tuple(
        "Point".to_string(),
        vec!["x".to_string(), "y".to_string()],
        vec![MontyObject::int(1), MontyObject::int(2)],
    );
    let b = a.clone();
    assert_eq!(a, b);
}

#[test]
fn eq_int_and_bigint() {
    assert_eq!(MontyObject::int(7), MontyObject::bigint(7.into()));
    assert_ne!(
        MontyObject::int(7),
        MontyObject::bigint("123456789012345678901234567890".parse().unwrap())
    );
}

#[test]
fn eq_float_by_bits() {
    assert_eq!(MontyObject::float(f64::NAN), MontyObject::float(f64::NAN));
    assert_ne!(MontyObject::float(0.0), MontyObject::float(-0.0));
}

#[test]
fn eq_named_tuple_with_tuple() {
    let named = MontyObject::named_tuple("Point", ["x", "y"], [MontyObject::int(1), MontyObject::int(2)]);
    assert_eq!(named, MontyObject::tuple([MontyObject::int(1), MontyObject::int(2)]));
    assert_ne!(named, MontyObject::list([MontyObject::int(1), MontyObject::int(2)]));
}

#[test]
fn eq_ignores_arena_layout() {
    // one value with sharing, one with copies, one with an unreachable node
    let mut graph = MontyGraph::new();
    let one = graph.push(MontyNode::Int(1));
    let inner = graph.push(MontyNode::List(vec![one]));
    let root = graph.push(MontyNode::List(vec![inner, inner]));
    let shared = MontyObject::new(graph, root).unwrap();
    let copied = MontyObject::list([
        MontyObject::list([MontyObject::int(1)]),
        MontyObject::list([MontyObject::int(1)]),
    ]);
    let mut padded = copied.clone();
    padded.graph.merge(MontyObject::string("unreachable").graph);
    assert_ne!(shared.graph.len(), copied.graph.len());
    assert_eq!(shared, copied);
    assert_eq!(copied, padded);
    assert_ne!(
        shared,
        MontyObject::list([MontyObject::list([MontyObject::int(1)]), MontyObject::list([])])
    );
}

#[test]
fn eq_class_instances() {
    let class = || MontyObject::class_type("Foo", MontyUuid::from_u128(1), true, false, []);
    let a = MontyObject::class_instance(
        class(),
        MontyUuid::from_u128(2),
        [(MontyObject::string("x"), MontyObject::int(1))],
    );
    assert_eq!(a, a.clone());
    let other_id = MontyObject::class_instance(
        class(),
        MontyUuid::from_u128(3),
        [(MontyObject::string("x"), MontyObject::int(1))],
    );
    assert_ne!(a, other_id);
    let other_attrs = MontyObject::class_instance(class(), MontyUuid::from_u128(2), []);
    assert_ne!(a, other_attrs);
}

// === accessors and Display ===

#[test]
fn accessors_read_leaves_and_containers() {
    let value = MontyObject::dict([(
        MontyObject::string("k"),
        MontyObject::list([MontyObject::int(1), MontyObject::bool(true)]),
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
    let text = MontyObject::string("hi");
    assert_eq!(text.to_string(), "hi");
    assert_eq!(text.py_repr(), "'hi'");
    let list = MontyObject::list([text, MontyObject::none(), MontyObject::cycle("[...]")]);
    assert_eq!(list.to_string(), "['hi', None, [...]]");
    assert_eq!(
        MontyObject::class_type("Foo", MontyUuid::from_u128(1), true, false, []).to_string(),
        "<class 'Foo'>"
    );
    let instance = MontyObject::class_instance(
        MontyObject::class_type("Foo", MontyUuid::from_u128(1), true, false, []),
        MontyUuid::from_u128(2),
        [(MontyObject::string("x"), MontyObject::int(1))],
    );
    assert_eq!(instance.py_repr(), "Foo(x=1)");
    assert_eq!(instance.type_name(), "Foo");
}
