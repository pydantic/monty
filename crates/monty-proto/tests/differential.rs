//! Differential tests proving the hand-written `WireObject` codec
//! (`src/wire.rs`) is byte-for-byte compatible with prost's generated
//! encoding of the same `.proto` schema.
//!
//! `tests/oracle/monty.v1.rs` is a fully prost-generated mirror of the schema
//! (regenerated alongside the protocol code by `make generate-proto`, kept in
//! sync by `make check-proto`). [`to_oracle`] independently maps each
//! `MontyObject` onto the mirror, so for every corpus value there are two
//! completely separate encode paths whose bytes must agree, and two decode
//! paths that must reconstruct the same value.
//!
//! The oracle is also the tool for crafting *hostile* frames: values that are
//! structurally valid protobuf but semantically invalid (out-of-range dates,
//! unknown enum names) which the hand-written decoder must reject — that
//! validation now happens during decode, so these tests pin the exact error
//! messages a misbehaving peer produces.

use monty::MontyRun;
use monty_proto::{WireFunctionCall, WireObject, pb};
use monty_types::{
    CompileOptions, DictPairs, ExcType, MontyClassInstance, MontyClassType, MontyDate, MontyDateTime, MontyFileHandle,
    MontyObject, MontyTime, MontyTimeDelta, MontyTimeZone, MontyType, MontyUuid,
};
use num_bigint::{BigInt, Sign};
use prost::Message;

use crate::oracle::monty_object::Kind;

#[path = "oracle/monty.v1.rs"]
mod oracle;

/// Every `MontyObject` shape, deliberately including protobuf-default
/// payloads (zeros, empty strings, `false`, empty containers, `Some("")`)
/// where prost's implicit/explicit field-presence rules diverge most.
fn corpus() -> Vec<MontyObject> {
    let bigint: BigInt = "123456789012345678901234567890123456789".parse().unwrap();
    vec![
        MontyObject::Ellipsis,
        MontyObject::NotImplemented,
        MontyObject::None,
        MontyObject::Bool(false), // oneof arms encode even at default payloads
        MontyObject::Bool(true),
        MontyObject::Int(0),
        MontyObject::Int(-1),
        MontyObject::Int(i64::MIN),
        MontyObject::Int(i64::MAX),
        MontyObject::BigInt(BigInt::ZERO),
        MontyObject::BigInt(bigint.clone()),
        MontyObject::BigInt(-bigint),
        MontyObject::Float(0.0),
        MontyObject::Float(-0.0),
        MontyObject::Float(f64::NAN),
        MontyObject::Float(f64::NEG_INFINITY),
        MontyObject::String(String::new()),
        MontyObject::String("héllo \u{1F40D}".to_owned()),
        MontyObject::Bytes(vec![]),
        MontyObject::Bytes(vec![0, 255, 128]),
        MontyObject::List(vec![]),
        MontyObject::List(vec![
            MontyObject::Int(1),
            MontyObject::String("two".to_owned()),
            MontyObject::List(vec![MontyObject::None]),
        ]),
        MontyObject::Tuple(vec![MontyObject::Bool(true), MontyObject::Float(2.5)]),
        MontyObject::Set(vec![MontyObject::Int(1), MontyObject::Int(2)]),
        MontyObject::FrozenSet(vec![MontyObject::String("a".to_owned())]),
        MontyObject::NamedTuple {
            type_name: String::new(),
            field_names: vec![],
            values: vec![],
        },
        MontyObject::NamedTuple {
            type_name: "os.stat_result".to_owned(),
            field_names: vec!["st_mode".to_owned(), String::new()],
            values: vec![MontyObject::Int(0o644), MontyObject::None],
        },
        MontyObject::dict(Vec::new()),
        MontyObject::dict(vec![
            (MontyObject::Int(1), MontyObject::String("one".to_owned())),
            (
                MontyObject::Tuple(vec![MontyObject::Int(1), MontyObject::Int(2)]),
                MontyObject::None,
            ),
        ]),
        MontyObject::Date(MontyDate {
            year: 2026,
            month: 6,
            day: 12,
        }),
        // a midnight datetime: every time component is a protobuf default
        MontyObject::DateTime(MontyDateTime {
            year: 1,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            offset_seconds: None,
            timezone_name: None,
        }),
        // every field at its implicit-presence default: nothing but the
        // submessage key should reach the wire
        MontyObject::Time(MontyTime {
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            offset_seconds: None,
            timezone_name: None,
            fold: 0,
        }),
        MontyObject::Time(MontyTime {
            hour: 23,
            minute: 59,
            second: 59,
            microsecond: 999_999,
            offset_seconds: Some(0),
            timezone_name: Some(String::new()),
            fold: 1,
        }),
        MontyObject::Time(MontyTime {
            hour: 1,
            minute: 2,
            second: 3,
            microsecond: 4,
            offset_seconds: Some(-3600),
            timezone_name: Some("MINUS1".to_owned()),
            fold: 0,
        }),
        // explicit-presence edge: offset of exactly 0 and an empty name must
        // still encode (proto3 `optional`), unlike implicit-presence fields
        MontyObject::DateTime(MontyDateTime {
            year: 2026,
            month: 6,
            day: 12,
            hour: 23,
            minute: 59,
            second: 58,
            microsecond: 999_999,
            offset_seconds: Some(0),
            timezone_name: Some(String::new()),
        }),
        MontyObject::TimeDelta(MontyTimeDelta {
            days: 0,
            seconds: 0,
            microseconds: 0,
        }),
        MontyObject::TimeDelta(MontyTimeDelta {
            days: -2,
            seconds: 86_399,
            microseconds: 999_999,
        }),
        MontyObject::TimeZone(MontyTimeZone {
            offset_seconds: 0,
            name: None,
        }),
        MontyObject::TimeZone(MontyTimeZone {
            offset_seconds: -19_800,
            name: Some("IST".to_owned()),
        }),
        MontyObject::Exception {
            exc_type: ExcType::ValueError,
            arg: None,
        },
        MontyObject::Exception {
            exc_type: ExcType::JsonDecodeError,
            arg: Some(String::new()),
        },
        MontyObject::Type(MontyType::Int),
        MontyObject::Type(MontyType::Exception(ExcType::KeyError)),
        MontyObject::Type(MontyType::Instance(Box::new(MontyClassType {
            name: "Foo".to_owned(),
            id: MontyUuid::from_u128(0xF00),
            host_defined: false,
            is_dataclass: false,
            attrs: DictPairs::default(),
        }))),
        MontyObject::Type(MontyType::Instance(Box::new(MontyClassType {
            name: "Child".to_owned(),
            id: MontyUuid::from_u128(0xF01),
            host_defined: true,
            is_dataclass: true,
            attrs: vec![
                (MontyObject::String("SIDES".to_owned()), MontyObject::Int(4)),
                (
                    MontyObject::String("KIND".to_owned()),
                    MontyObject::String("polygon".to_owned()),
                ),
            ]
            .into(),
        }))),
        MontyObject::builtin_function_from_name("len").expect("len is a builtin"),
        MontyObject::Path(String::new()),
        MontyObject::Path("/mnt/data/file.txt".to_owned()),
        MontyObject::FileHandle(MontyFileHandle {
            path: "/f.bin".to_owned(),
            mode: "rb".parse().unwrap(),
            position: 0,
        }),
        MontyObject::ClassInstance(Box::new(MontyClassInstance {
            class_type: MontyClassType {
                name: String::new(),
                id: MontyUuid::from_u128(0),
                host_defined: false,
                is_dataclass: false,
                attrs: DictPairs::default(),
            },
            instance_id: MontyUuid::from_u128(0),
            attrs: DictPairs::from(Vec::new()),
        })),
        MontyObject::ClassInstance(Box::new(MontyClassInstance {
            class_type: MontyClassType {
                name: "Point".to_owned(),
                id: MontyUuid::from_u128(0xDEAD_BEEF),
                host_defined: true,
                is_dataclass: true,
                attrs: DictPairs::default(),
            },
            instance_id: MontyUuid::from_u128(0xFEED_FACE),
            attrs: DictPairs::from(vec![
                (MontyObject::String("x".to_owned()), MontyObject::Int(1)),
                (MontyObject::String("y".to_owned()), MontyObject::Int(2)),
            ]),
        })),
        // an instance whose class branch carries eager class attrs
        MontyObject::ClassInstance(Box::new(MontyClassInstance {
            class_type: MontyClassType {
                name: "Square".to_owned(),
                id: MontyUuid::from_u128(0xF02),
                host_defined: true,
                is_dataclass: false,
                attrs: vec![
                    (MontyObject::String("SIDES".to_owned()), MontyObject::Int(4)),
                    (MontyObject::String(String::new()), MontyObject::None),
                ]
                .into(),
            },
            instance_id: MontyUuid::from_u128(0xF03),
            attrs: DictPairs::from(vec![(MontyObject::String("size".to_owned()), MontyObject::Int(3))]),
        })),
        MontyObject::Function {
            name: "f".to_owned(),
            docstring: None,
        },
        MontyObject::Function {
            name: "fetch".to_owned(),
            docstring: Some(String::new()),
        },
        MontyObject::Repr(String::new()),
        MontyObject::Repr("<unrepresentable>".to_owned()),
        MontyObject::Cycle(0, "[...]".to_owned()), // zero identity is a default-skipped field
        MontyObject::Cycle(7, "{...}".to_owned()),
    ]
}

/// Independent `MontyObject` → oracle mapping (the encode path the generated
/// code would have used). Deliberately *not* shared with `src/wire.rs` — the
/// whole point is two implementations that can disagree.
fn to_oracle(obj: &MontyObject) -> oracle::MontyObject {
    let kind = match obj {
        MontyObject::Ellipsis => Kind::Ellipsis(oracle::Unit {}),
        MontyObject::NotImplemented => Kind::NotImplemented(oracle::Unit {}),
        MontyObject::None => Kind::None(oracle::Unit {}),
        MontyObject::Bool(b) => Kind::Boolean(*b),
        MontyObject::Int(i) => Kind::Int(*i),
        MontyObject::BigInt(bi) => {
            let (sign, magnitude) = bi.to_bytes_be();
            Kind::Bigint(oracle::BigInt {
                negative: sign == Sign::Minus,
                magnitude,
            })
        }
        MontyObject::Float(f) => Kind::Float(*f),
        MontyObject::String(s) => Kind::Str(s.clone()),
        MontyObject::Bytes(b) => Kind::Bytes(b.clone()),
        MontyObject::List(items) => Kind::List(oracle_list(items)),
        MontyObject::Tuple(items) => Kind::Tuple(oracle_list(items)),
        MontyObject::NamedTuple {
            type_name,
            field_names,
            values,
        } => Kind::NamedTuple(oracle::NamedTuple {
            type_name: type_name.clone(),
            field_names: field_names.clone(),
            values: values.iter().map(to_oracle).collect(),
        }),
        MontyObject::Dict(pairs) => Kind::Dict(oracle_dict(pairs)),
        MontyObject::Set(items) => Kind::Set(oracle_list(items)),
        MontyObject::FrozenSet(items) => Kind::FrozenSet(oracle_list(items)),
        MontyObject::Date(d) => Kind::Date(oracle::Date {
            year: d.year,
            month: u32::from(d.month),
            day: u32::from(d.day),
        }),
        MontyObject::DateTime(dt) => Kind::Datetime(oracle::DateTime {
            year: dt.year,
            month: u32::from(dt.month),
            day: u32::from(dt.day),
            hour: u32::from(dt.hour),
            minute: u32::from(dt.minute),
            second: u32::from(dt.second),
            microsecond: dt.microsecond,
            offset_seconds: dt.offset_seconds,
            timezone_name: dt.timezone_name.clone(),
        }),
        MontyObject::Time(t) => Kind::Time(oracle::Time {
            hour: u32::from(t.hour),
            minute: u32::from(t.minute),
            second: u32::from(t.second),
            microsecond: t.microsecond,
            offset_seconds: t.offset_seconds,
            timezone_name: t.timezone_name.clone(),
            fold: u32::from(t.fold),
        }),
        MontyObject::TimeDelta(td) => Kind::Timedelta(oracle::TimeDelta {
            days: td.days,
            seconds: td.seconds,
            microseconds: td.microseconds,
        }),
        MontyObject::TimeZone(tz) => Kind::Timezone(oracle::TimeZone {
            offset_seconds: tz.offset_seconds,
            name: tz.name.clone(),
        }),
        MontyObject::Exception { exc_type, arg } => Kind::Exception(oracle::Exception {
            exc_type: exc_type.to_string(),
            arg: arg.clone(),
        }),
        MontyObject::Type(t) => Kind::Type(oracle_type(t)),
        MontyObject::BuiltinFunction(bf) => Kind::BuiltinFunction(bf.to_string()),
        MontyObject::Path(p) => Kind::Path(p.clone()),
        MontyObject::FileHandle(fh) => Kind::FileHandle(oracle::FileHandle {
            path: fh.path.clone(),
            mode: fh.mode.as_str().to_owned(),
            position: fh.position,
        }),
        MontyObject::ClassInstance(instance) => Kind::ClassInstance(oracle::ClassInstance {
            r#type: Some(oracle_class_type(&instance.class_type)),
            instance_id: Some(oracle_uuid(&instance.instance_id)),
            attrs: Some(oracle_dict(&instance.attrs)),
        }),
        MontyObject::Function { name, docstring } => Kind::Function(oracle::Function {
            name: name.clone(),
            docstring: docstring.clone(),
        }),
        MontyObject::Repr(r) => Kind::Repr(r.clone()),
        MontyObject::Cycle(identity, placeholder) => Kind::Cycle(oracle::Cycle {
            identity: *identity as u64,
            placeholder: placeholder.clone(),
        }),
    };
    oracle::MontyObject { kind: Some(kind) }
}

/// Oracle mirror of a `MontyType` as the wire `Type` message.
fn oracle_type(t: &MontyType) -> oracle::Type {
    match t {
        MontyType::Instance(class_type) => oracle_class_type(class_type),
        other => oracle::Type {
            name: other.to_string(),
            origin: oracle::TypeOrigin::Builtin as i32,
            ..oracle::Type::default()
        },
    }
}

/// Oracle mirror of a `MontyClassType`.
fn oracle_class_type(class_type: &MontyClassType) -> oracle::Type {
    let origin = if class_type.host_defined {
        oracle::TypeOrigin::Host
    } else {
        oracle::TypeOrigin::Sandbox
    };
    let attrs = if class_type.attrs.is_empty() {
        None
    } else {
        Some(oracle_dict(&class_type.attrs))
    };
    oracle::Type {
        name: class_type.name.clone(),
        id: Some(oracle_uuid(&class_type.id)),
        origin: origin as i32,
        is_dataclass: class_type.is_dataclass,
        attrs,
    }
}

/// Oracle mirror of a `MontyUuid`.
fn oracle_uuid(uuid: &MontyUuid) -> oracle::Uuid {
    oracle::Uuid {
        data: uuid.as_bytes().to_vec(),
    }
}

fn oracle_list(items: &[MontyObject]) -> oracle::ObjectList {
    oracle::ObjectList {
        items: items.iter().map(to_oracle).collect(),
    }
}

fn oracle_dict(pairs: &DictPairs) -> oracle::Dict {
    oracle::Dict {
        pairs: oracle_pairs(pairs),
    }
}

fn oracle_pairs<'a>(pairs: impl IntoIterator<Item = &'a (MontyObject, MontyObject)>) -> Vec<oracle::Pair> {
    pairs
        .into_iter()
        .map(|(key, value)| oracle::Pair {
            key: Some(to_oracle(key)),
            value: Some(to_oracle(value)),
        })
        .collect()
}

/// Decodes wire bytes through the hand-written codec.
fn decode_wire(bytes: &[u8]) -> Result<MontyObject, String> {
    let wire = WireObject::decode(bytes).map_err(|err| err.to_string())?;
    wire.into_object().map_err(|err| err.to_string())
}

// ============================================================================
// Byte compatibility on valid values
// ============================================================================

#[test]
fn hand_encoding_matches_generated_encoding() {
    for obj in corpus() {
        let hand = WireObject::new(obj.clone()).encode_to_vec();
        let generated = to_oracle(&obj).encode_to_vec();
        assert_eq!(hand, generated, "encodings diverge for {obj:?}");
    }
}

#[test]
fn hand_decoder_reads_generated_bytes() {
    for obj in corpus() {
        let generated = to_oracle(&obj).encode_to_vec();
        let back = decode_wire(&generated).expect("decode failed");
        assert_eq!(back, obj, "decoding generated bytes diverges for {obj:?}");
    }
}

#[test]
fn generated_decoder_reads_hand_bytes() {
    for obj in corpus() {
        let hand = WireObject::new(obj.clone()).encode_to_vec();
        let back = oracle::MontyObject::decode(hand.as_slice()).expect("oracle decode failed");
        // compare re-encoded bytes rather than structs: the oracle's derived
        // PartialEq uses IEEE float semantics, under which NaN != NaN
        assert_eq!(
            back.encode_to_vec(),
            hand,
            "oracle decoding hand bytes diverges for {obj:?}"
        );
    }
}

#[test]
fn hand_call_payloads_match_generated_encoding() {
    let args = vec![
        MontyObject::Int(1),
        MontyObject::String("arg".to_owned()),
        MontyObject::List(vec![MontyObject::None]),
    ];
    let kwargs = vec![
        (MontyObject::String("flag".to_owned()), MontyObject::Bool(true)),
        (MontyObject::String("count".to_owned()), MontyObject::Int(3)),
    ];

    // Both receiver states: a routed call (method / `__call__`) and a plain
    // external call (absent field).
    let receivers = [Some(MontyUuid::from_u128(7)), None];
    for object_id in receivers {
        let oracle_object_id = object_id.map(|uuid| oracle_uuid(&uuid));
        let hand_call = WireFunctionCall {
            function_name: "external".to_owned(),
            args: args.clone(),
            kwargs: kwargs.clone(),
            call_id: 42,
            object_id,
        };
        let generated_call = oracle::FunctionCall {
            function_name: "external".to_owned(),
            args: args.iter().map(to_oracle).collect(),
            kwargs: oracle_pairs(&kwargs),
            call_id: 42,
            object_id: oracle_object_id,
        };
        assert_eq!(hand_call.encode_to_vec(), generated_call.encode_to_vec());
        assert_eq!(
            WireFunctionCall::decode(generated_call.encode_to_vec().as_slice())
                .expect("generated function call decodes"),
            hand_call
        );
        assert_eq!(
            oracle::FunctionCall::decode(hand_call.encode_to_vec().as_slice())
                .expect("hand function call decodes")
                .encode_to_vec(),
            generated_call.encode_to_vec()
        );
    }

    // `OsCall` is fully generated, but its `Getenv.default` field embeds the
    // hand-written `WireObject` — check the embedding agrees with the oracle
    // byte-for-byte.
    let default = MontyObject::List(vec![MontyObject::None, MontyObject::Int(3)]);
    let hand_os = pb::OsCall {
        call_id: 7,
        call: Some(pb::os_call::Call::Getenv(pb::os_call::Getenv {
            key: "HOME".to_owned(),
            default: Some(WireObject::new(default.clone())),
        })),
    };
    let generated_os = oracle::OsCall {
        call_id: 7,
        call: Some(oracle::os_call::Call::Getenv(oracle::os_call::Getenv {
            key: "HOME".to_owned(),
            default: Some(to_oracle(&default)),
        })),
    };
    assert_eq!(hand_os.encode_to_vec(), generated_os.encode_to_vec());
    assert_eq!(
        pb::OsCall::decode(generated_os.encode_to_vec().as_slice()).expect("generated os call decodes"),
        hand_os
    );

    // `DateTimeNow` is fully typed (optional TimeZone) — no `WireObject`
    // embedding, but keep the byte-compat check against the oracle.
    let hand_now = pb::OsCall {
        call_id: 9,
        call: Some(pb::os_call::Call::DateTimeNow(pb::os_call::DateTimeNow {
            tz: Some(pb::TimeZone {
                offset_seconds: 3600,
                name: Some("CET".to_owned()),
            }),
        })),
    };
    let generated_now = oracle::OsCall {
        call_id: 9,
        call: Some(oracle::os_call::Call::DateTimeNow(oracle::os_call::DateTimeNow {
            tz: Some(oracle::TimeZone {
                offset_seconds: 3600,
                name: Some("CET".to_owned()),
            }),
        })),
    };
    assert_eq!(hand_now.encode_to_vec(), generated_now.encode_to_vec());
    assert_eq!(
        pb::OsCall::decode(generated_now.encode_to_vec().as_slice()).expect("generated now call decodes"),
        hand_now
    );
}

/// A cyclic value produced by real execution must agree byte-for-byte too —
/// it exercises the `Cycle` placeholder arm end to end.
#[test]
fn executed_cycle_value_is_byte_compatible() {
    let run = MontyRun::new(
        "a = []\na.append(a)\na".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    let cyclic = run.run_no_limits(vec![]).unwrap();
    let hand = WireObject::new(cyclic.clone()).encode_to_vec();
    assert_eq!(hand, to_oracle(&cyclic).encode_to_vec());
    assert_eq!(decode_wire(&hand).expect("decode failed"), cyclic);
}

// ============================================================================
// Hostile frames: semantic validation happens during decode
// ============================================================================

/// Encodes an oracle `kind` arm and decodes it through the hand-written
/// codec, returning the decode error message.
fn rejected(kind: oracle::monty_object::Kind) -> String {
    let bytes = oracle::MontyObject { kind: Some(kind) }.encode_to_vec();
    decode_wire(&bytes).expect_err("hostile frame must be rejected")
}

#[test]
fn invalid_values_are_rejected_during_decode() {
    assert_eq!(
        rejected(Kind::Exception(oracle::Exception {
            exc_type: "NotARealError".to_owned(),
            arg: None,
        })),
        "failed to decode Protobuf message: unknown exception type \"NotARealError\""
    );
    assert_eq!(
        rejected(Kind::Type(oracle::Type {
            name: "NotAType".to_owned(),
            origin: oracle::TypeOrigin::Builtin as i32,
            ..oracle::Type::default()
        })),
        "failed to decode Protobuf message: unknown type name \"NotAType\""
    );
    // origin must be specified
    assert_eq!(
        rejected(Kind::Type(oracle::Type {
            name: "int".to_owned(),
            ..oracle::Type::default()
        })),
        "failed to decode Protobuf message: invalid value for Type: origin must be specified"
    );
    // a builtin must not carry an id
    assert_eq!(
        rejected(Kind::Type(oracle::Type {
            name: "int".to_owned(),
            origin: oracle::TypeOrigin::Builtin as i32,
            id: Some(oracle::Uuid { data: vec![0; 16] }),
            ..oracle::Type::default()
        })),
        "failed to decode Protobuf message: invalid value for Type: a builtin type must not carry an id"
    );
    assert_eq!(
        rejected(Kind::Type(oracle::Type {
            name: "int".to_owned(),
            origin: oracle::TypeOrigin::Builtin as i32,
            attrs: Some(oracle::Dict::default()),
            ..oracle::Type::default()
        })),
        "failed to decode Protobuf message: invalid value for Type: a builtin type must not carry attrs"
    );
    // a class type must carry an id
    assert_eq!(
        rejected(Kind::Type(oracle::Type {
            name: "Foo".to_owned(),
            origin: oracle::TypeOrigin::Host as i32,
            ..oracle::Type::default()
        })),
        "failed to decode Protobuf message: invalid value for Type: a class type must carry an id"
    );
    // a uuid must be exactly 16 bytes
    assert_eq!(
        rejected(Kind::Type(oracle::Type {
            name: "Foo".to_owned(),
            origin: oracle::TypeOrigin::Sandbox as i32,
            id: Some(oracle::Uuid { data: vec![0; 5] }),
            ..oracle::Type::default()
        })),
        "failed to decode Protobuf message: invalid value for Type.id: uuid must be 16 bytes, got 5"
    );
    // a class instance's type must not be a builtin
    assert_eq!(
        rejected(Kind::ClassInstance(oracle::ClassInstance {
            r#type: Some(oracle::Type {
                name: "int".to_owned(),
                origin: oracle::TypeOrigin::Builtin as i32,
                ..oracle::Type::default()
            }),
            instance_id: Some(oracle::Uuid { data: vec![0; 16] }),
            attrs: Some(oracle::Dict::default()),
        })),
        "failed to decode Protobuf message: invalid value for ClassInstance.type: must be a class type, not a builtin"
    );
    // instance_id is required
    assert_eq!(
        rejected(Kind::ClassInstance(oracle::ClassInstance {
            r#type: Some(oracle::Type {
                name: "Foo".to_owned(),
                origin: oracle::TypeOrigin::Sandbox as i32,
                id: Some(oracle::Uuid { data: vec![7; 16] }),
                ..oracle::Type::default()
            }),
            instance_id: None,
            attrs: Some(oracle::Dict::default()),
        })),
        "failed to decode Protobuf message: missing required field ClassInstance.instance_id"
    );
    // attrs is required, even when empty
    assert_eq!(
        rejected(Kind::ClassInstance(oracle::ClassInstance {
            r#type: Some(class_type("Foo")),
            instance_id: Some(oracle::Uuid { data: vec![7; 16] }),
            attrs: None,
        })),
        "failed to decode Protobuf message: missing required field ClassInstance.attrs"
    );
    // the instance uuid must be exactly 16 bytes
    assert_eq!(
        rejected(Kind::ClassInstance(oracle::ClassInstance {
            r#type: Some(class_type("Foo")),
            instance_id: Some(oracle::Uuid { data: vec![7; 17] }),
            attrs: Some(oracle::Dict::default()),
        })),
        "failed to decode Protobuf message: invalid value for ClassInstance.instance_id: uuid must be 16 bytes, got 17"
    );
    // an origin outside the enum is rejected rather than defaulted
    assert_eq!(
        rejected(Kind::Type(oracle::Type {
            origin: 99,
            ..class_type("Foo")
        })),
        "failed to decode Protobuf message: invalid value for Type.origin: unknown origin 99"
    );
    assert_eq!(
        rejected(Kind::BuiltinFunction("not_a_builtin".to_owned())),
        "failed to decode Protobuf message: unknown builtin function \"not_a_builtin\""
    );
    // update file modes are not yet supported by monty's parser
    assert_eq!(
        rejected(Kind::FileHandle(oracle::FileHandle {
            path: "/f".to_owned(),
            mode: "r+".to_owned(),
            position: 0,
        })),
        "failed to decode Protobuf message: invalid file mode \"r+\""
    );
    // timezone_name without offset_seconds
    assert_eq!(
        rejected(Kind::Datetime(oracle::DateTime {
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
        "failed to decode Protobuf message: invalid value for DateTime.timezone_name: timezone_name requires offset_seconds"
    );
    // the uuid arm is declared in the schema but not yet implemented: the
    // hand-written decoder skips it like any unknown tag, leaving no kind
    assert_eq!(
        rejected(Kind::Uuid(oracle::Uuid { data: vec![7; 16] })),
        "missing required field MontyObject.kind"
    );
    // an absent kind decodes (it is a valid empty message) but cannot be
    // unwrapped into a value
    let empty = oracle::MontyObject { kind: None }.encode_to_vec();
    assert_eq!(
        decode_wire(&empty).expect_err("empty kind must be rejected"),
        "missing required field MontyObject.kind"
    );
}

/// A sandbox-origin `Foo` class type with a fixed id and no attrs.
fn class_type(name: &str) -> oracle::Type {
    oracle::Type {
        name: name.to_owned(),
        origin: oracle::TypeOrigin::Sandbox as i32,
        id: Some(oracle::Uuid { data: vec![7; 16] }),
        ..oracle::Type::default()
    }
}

/// The hand encoder emits eager class attrs only when non-empty, but a peer
/// may send the field present and empty: that decodes to the same value.
#[test]
fn present_but_empty_class_attrs_decode_as_absent() {
    let expected = MontyClassType {
        name: "Foo".to_owned(),
        id: MontyUuid::from_u128(0x0707_0707_0707_0707_0707_0707_0707_0707),
        host_defined: false,
        is_dataclass: false,
        attrs: DictPairs::default(),
    };
    let with_empty_attrs = oracle::Type {
        attrs: Some(oracle::Dict::default()),
        ..class_type("Foo")
    };
    let bytes = oracle::MontyObject {
        kind: Some(Kind::Type(with_empty_attrs.clone())),
    }
    .encode_to_vec();
    assert_eq!(
        decode_wire(&bytes).unwrap(),
        MontyObject::Type(MontyType::Instance(Box::new(expected.clone())))
    );
    let bytes = oracle::MontyObject {
        kind: Some(Kind::ClassInstance(oracle::ClassInstance {
            r#type: Some(with_empty_attrs),
            instance_id: Some(oracle::Uuid { data: vec![7; 16] }),
            attrs: Some(oracle::Dict::default()),
        })),
    }
    .encode_to_vec();
    assert_eq!(
        decode_wire(&bytes).unwrap(),
        MontyObject::ClassInstance(Box::new(MontyClassInstance {
            class_type: expected,
            instance_id: MontyUuid::from_u128(0x0707_0707_0707_0707_0707_0707_0707_0707),
            attrs: DictPairs::default(),
        }))
    );
}

/// The wire is untrusted: temporal values that fit their integer fields but
/// violate the semantic invariants documented on `MontyDate`/`MontyDateTime`/
/// `MontyTimeDelta` must be rejected during decode.
#[test]
fn out_of_range_temporal_values_are_rejected() {
    let date = |year, month, day| Kind::Date(oracle::Date { year, month, day });
    let rejected_field =
        |kind, expected_field: &str| rejected(kind).contains(&format!("invalid value for {expected_field}:"));
    assert!(rejected_field(date(0, 1, 1), "Date.year"));
    assert!(rejected_field(date(10_000, 1, 1), "Date.year"));
    assert!(rejected_field(date(2026, 0, 1), "Date.month"));
    assert!(rejected_field(date(2026, 13, 1), "Date.month"));
    assert!(rejected_field(date(2026, 2, 0), "Date.day"));
    assert!(rejected_field(date(2026, 2, 29), "Date.day")); // 2026 is not a leap year
    assert!(rejected_field(date(2025, 4, 31), "Date.day"));
    assert_eq!(
        rejected(date(2026, 4096, 1)),
        "failed to decode Protobuf message: invalid value for Date.month: 4096 is outside the range 1..=12"
    );

    let datetime = |hour, minute, second, microsecond| {
        Kind::Datetime(oracle::DateTime {
            year: 2026,
            month: 1,
            day: 1,
            hour,
            minute,
            second,
            microsecond,
            offset_seconds: None,
            timezone_name: None,
        })
    };
    assert!(rejected_field(datetime(24, 0, 0, 0), "DateTime.hour"));
    assert!(rejected_field(datetime(0, 60, 0, 0), "DateTime.minute"));
    assert!(rejected_field(datetime(0, 0, 60, 0), "DateTime.second"));
    assert!(rejected_field(datetime(0, 0, 0, 1_000_000), "DateTime.microsecond"));

    let time = |hour, minute, second, microsecond, fold| {
        Kind::Time(oracle::Time {
            hour,
            minute,
            second,
            microsecond,
            offset_seconds: None,
            timezone_name: None,
            fold,
        })
    };
    assert!(rejected_field(time(24, 0, 0, 0, 0), "Time.hour"));
    assert!(rejected_field(time(0, 60, 0, 0, 0), "Time.minute"));
    assert!(rejected_field(time(0, 0, 60, 0, 0), "Time.second"));
    assert!(rejected_field(time(0, 0, 0, 1_000_000, 0), "Time.microsecond"));
    assert!(rejected_field(time(0, 0, 0, 0, 2), "Time.fold"));

    let timedelta = |seconds, microseconds| {
        Kind::Timedelta(oracle::TimeDelta {
            days: 1,
            seconds,
            microseconds,
        })
    };
    assert!(rejected_field(timedelta(-1, 0), "TimeDelta.seconds"));
    assert!(rejected_field(timedelta(86_400, 0), "TimeDelta.seconds"));
    assert!(rejected_field(timedelta(0, -1), "TimeDelta.microseconds"));
    assert!(rejected_field(timedelta(0, 1_000_000), "TimeDelta.microseconds"));

    // an offset must be strictly inside ±24 hours, as `datetime.timezone` requires
    let aware_datetime = |offset_seconds| {
        Kind::Datetime(oracle::DateTime {
            year: 2026,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            offset_seconds: Some(offset_seconds),
            timezone_name: None,
        })
    };
    let aware_time = |offset_seconds| {
        Kind::Time(oracle::Time {
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            offset_seconds: Some(offset_seconds),
            timezone_name: None,
            fold: 0,
        })
    };
    assert!(rejected_field(aware_datetime(86_400), "DateTime.offset_seconds"));
    assert!(rejected_field(aware_datetime(-86_400), "DateTime.offset_seconds"));
    assert!(rejected_field(aware_time(86_400), "Time.offset_seconds"));
    assert!(rejected_field(aware_time(-86_400), "Time.offset_seconds"));
    assert_eq!(
        rejected(Kind::Timezone(oracle::TimeZone {
            offset_seconds: 86_400,
            name: None,
        })),
        "failed to decode Protobuf message: invalid value for TimeZone.offset_seconds: 86400 is outside the range -86399..=86399"
    );
}

// ============================================================================
// Wire-level behaviours
// ============================================================================

/// Unknown fields must be skipped (forward compatibility), exactly like
/// prost's generated decoder.
#[test]
fn unknown_fields_are_skipped() {
    let mut bytes = WireObject::new(MontyObject::Int(42)).encode_to_vec();
    // append an unknown varint field: key = 99 << 3 | 0 = 792 (varint
    // 0x98 0x06), value 7
    bytes.extend_from_slice(&[0x98, 0x06, 0x07]);
    assert_eq!(
        decode_wire(&bytes).expect("unknown field must be skipped"),
        MontyObject::Int(42)
    );
}

/// Truncated and corrupt frames fail to decode rather than panicking.
#[test]
fn corrupt_frames_fail_cleanly() {
    let bytes = WireObject::new(MontyObject::List(vec![MontyObject::Int(1)])).encode_to_vec();
    for cut in 1..bytes.len() {
        assert!(decode_wire(&bytes[..cut]).is_err(), "truncation at {cut} must fail");
    }
}
