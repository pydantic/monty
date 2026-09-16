//! Differential tests proving the hand-written `WireArena` codec
//! (`src/wire.rs`) is byte-for-byte compatible with prost's generated
//! encoding of the same `.proto` schema.
//!
//! `tests/oracle/monty.v1.rs` is a fully prost-generated mirror of the schema
//! (regenerated alongside the protocol code by `make generate-proto`, kept in
//! sync by `make check-proto`). [`to_oracle`] independently maps each
//! `MontyGraph` onto the mirror, so for every corpus arena there are two
//! completely separate encode paths whose bytes must agree, and two decode
//! paths that must reconstruct the same arena.
//!
//! The oracle is also the tool for crafting *hostile* frames: values that are
//! structurally valid protobuf but semantically invalid (out-of-range dates,
//! unknown enum names) which the hand-written decoder must reject — that
//! validation now happens during decode, so these tests pin the exact error
//! messages a misbehaving peer produces.

use monty::{MontyRun, RunProgress};
use monty_proto::{WireArena, WireFunctionCall, os_call_to_proto, pb};
use monty_types::{
    CallArgs, ClassTypeNode, CompileOptions, ExcType, GetenvArgs, MontyDate, MontyDateTime, MontyFileHandle,
    MontyGraph, MontyNode, MontyObject, MontyTime, MontyTimeDelta, MontyTimeZone, MontyType, MontyUuid, NodeId,
    OsFunctionCall, PrintWriter, ResourceTracker,
};
use num_bigint::{BigInt, Sign};
use prost::Message;

use crate::oracle::monty_node::Kind;

#[path = "oracle/monty.v1.rs"]
mod oracle;

/// Every `MontyObject` shape, deliberately including protobuf-default
/// payloads (zeros, empty strings, `false`, empty containers, `Some("")`)
/// where prost's implicit/explicit field-presence rules diverge most.
fn corpus() -> Vec<MontyObject> {
    let bigint: BigInt = "123456789012345678901234567890123456789".parse().unwrap();
    vec![
        MontyObject::ellipsis(),
        MontyObject::not_implemented(),
        MontyObject::none(),
        MontyObject::bool(false), // oneof arms encode even at default payloads
        MontyObject::bool(true),
        MontyObject::int(0),
        MontyObject::int(-1),
        MontyObject::int(i64::MIN),
        MontyObject::int(i64::MAX),
        MontyObject::bigint(BigInt::ZERO),
        MontyObject::bigint(bigint.clone()),
        MontyObject::bigint(-bigint),
        MontyObject::float(0.0),
        MontyObject::float(-0.0),
        MontyObject::float(f64::NAN),
        MontyObject::float(f64::NEG_INFINITY),
        MontyObject::string(String::new()),
        MontyObject::string("héllo \u{1F40D}".to_owned()),
        MontyObject::bytes(vec![]),
        MontyObject::bytes(vec![0, 255, 128]),
        MontyObject::list([]),
        MontyObject::list([
            MontyObject::int(1),
            MontyObject::string("two".to_owned()),
            MontyObject::list([MontyObject::none()]),
        ]),
        MontyObject::tuple([MontyObject::bool(true), MontyObject::float(2.5)]),
        MontyObject::set([MontyObject::int(1), MontyObject::int(2)]),
        MontyObject::frozenset([MontyObject::string("a".to_owned())]),
        MontyObject::named_tuple(String::new(), Vec::<String>::new(), vec![]),
        MontyObject::named_tuple(
            "os.stat_result".to_owned(),
            vec!["st_mode".to_owned(), String::new()],
            vec![MontyObject::int(0o644), MontyObject::none()],
        ),
        MontyObject::dict(Vec::new()),
        MontyObject::dict([
            (MontyObject::int(1), MontyObject::string("one".to_owned())),
            (
                MontyObject::tuple([MontyObject::int(1), MontyObject::int(2)]),
                MontyObject::none(),
            ),
        ]),
        MontyObject::date(MontyDate {
            year: 2026,
            month: 6,
            day: 12,
        }),
        // a midnight datetime: every time component is a protobuf default
        MontyObject::datetime(MontyDateTime {
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
        MontyObject::time(MontyTime {
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            offset_seconds: None,
            timezone_name: None,
            fold: 0,
        }),
        MontyObject::time(MontyTime {
            hour: 23,
            minute: 59,
            second: 59,
            microsecond: 999_999,
            offset_seconds: Some(0),
            timezone_name: Some(String::new()),
            fold: 1,
        }),
        MontyObject::time(MontyTime {
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
        MontyObject::datetime(MontyDateTime {
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
        MontyObject::timedelta(MontyTimeDelta {
            days: 0,
            seconds: 0,
            microseconds: 0,
        }),
        MontyObject::timedelta(MontyTimeDelta {
            days: -2,
            seconds: 86_399,
            microseconds: 999_999,
        }),
        MontyObject::timezone(MontyTimeZone {
            offset_seconds: 0,
            name: None,
        }),
        MontyObject::timezone(MontyTimeZone {
            offset_seconds: -19_800,
            name: Some("IST".to_owned()),
        }),
        MontyObject::exception(ExcType::ValueError, None),
        MontyObject::exception(ExcType::JsonDecodeError, Some(String::new())),
        MontyObject::type_object(MontyType::Int),
        MontyObject::type_object(MontyType::Exception(ExcType::KeyError)),
        MontyObject::class_type("Foo", MontyUuid::from_u128(0xF00), false, false, []),
        MontyObject::class_type(
            "Child",
            MontyUuid::from_u128(0xF01),
            true,
            true,
            [
                (MontyObject::string("SIDES".to_owned()), MontyObject::int(4)),
                (
                    MontyObject::string("KIND".to_owned()),
                    MontyObject::string("polygon".to_owned()),
                ),
            ],
        ),
        MontyObject::builtin_function_from_name("len").expect("len is a builtin"),
        MontyObject::path(String::new()),
        MontyObject::path("/mnt/data/file.txt".to_owned()),
        MontyObject::file_handle(MontyFileHandle {
            path: "/f.bin".to_owned(),
            mode: "rb".parse().unwrap(),
            position: 0,
        }),
        MontyObject::class_instance(
            MontyObject::class_type(String::new(), MontyUuid::from_u128(0), false, false, []),
            MontyUuid::from_u128(0),
            [],
        ),
        MontyObject::class_instance(
            MontyObject::class_type("Point", MontyUuid::from_u128(0xDEAD_BEEF), true, true, []),
            MontyUuid::from_u128(0xFEED_FACE),
            [
                (MontyObject::string("x".to_owned()), MontyObject::int(1)),
                (MontyObject::string("y".to_owned()), MontyObject::int(2)),
            ],
        ),
        // an instance whose class branch carries eager class attrs
        MontyObject::class_instance(
            MontyObject::class_type(
                "Square",
                MontyUuid::from_u128(0xF02),
                true,
                false,
                [
                    (MontyObject::string("SIDES".to_owned()), MontyObject::int(4)),
                    (MontyObject::string(String::new()), MontyObject::none()),
                ],
            ),
            MontyUuid::from_u128(0xF03),
            [(MontyObject::string("size".to_owned()), MontyObject::int(3))],
        ),
        MontyObject::function("f".to_owned(), None),
        MontyObject::function("fetch".to_owned(), Some(String::new())),
        MontyObject::repr(String::new()),
        MontyObject::repr("<unrepresentable>".to_owned()),
        MontyObject::cycle("[...]".to_owned()),
        MontyObject::cycle("{...}".to_owned()),
    ]
}

/// Every corpus value as an arena, plus arenas only sharing can produce: a
/// doubling ladder, a cycle leaf, and a class node shared by two instances.
fn graphs() -> Vec<MontyGraph> {
    let mut graphs: Vec<MontyGraph> = corpus().into_iter().map(|value| value.graph).collect();
    let mut ladder = MontyGraph::new();
    let mut x = ladder.push(MontyNode::Int(0));
    for _ in 0..4 {
        x = ladder.push(MontyNode::List(vec![x, x]));
    }
    ladder.push(MontyNode::Dict(vec![(x, x)]));
    graphs.push(ladder);
    graphs.push(
        MontyGraph::from_nodes(vec![
            MontyNode::Cycle("(...)".to_owned()),
            MontyNode::Tuple(vec![NodeId(0), NodeId(0)]),
        ])
        .unwrap(),
    );
    let mut shared_class = MontyGraph::new();
    let class = shared_class.push(MontyNode::ClassType(Box::new(ClassTypeNode {
        name: "Point".to_owned(),
        id: MontyUuid::from_u128(0xF10),
        host_defined: true,
        is_dataclass: true,
        attrs: vec![],
    })));
    let one = shared_class.push(MontyNode::Int(1));
    let key = shared_class.push(MontyNode::String("x".to_owned()));
    let first = shared_class.push(MontyNode::ClassInstance {
        class_type: class,
        instance_id: MontyUuid::from_u128(0xF11),
        attrs: vec![(key, one)],
    });
    let second = shared_class.push(MontyNode::ClassInstance {
        class_type: class,
        instance_id: MontyUuid::from_u128(0xF12),
        attrs: vec![],
    });
    shared_class.push(MontyNode::List(vec![first, second, class]));
    graphs.push(shared_class);
    graphs
}

/// Independent `MontyGraph` → oracle mapping (the encode path the generated
/// code would have used). Deliberately *not* shared with `src/wire.rs` — the
/// whole point is two implementations that can disagree.
fn to_oracle(graph: &MontyGraph) -> oracle::Arena {
    oracle::Arena {
        node_count: u32::try_from(graph.len()).unwrap(),
        nodes: graph.nodes().iter().map(node_to_oracle).collect(),
    }
}

fn node_to_oracle(node: &MontyNode) -> oracle::MontyNode {
    let kind = match node {
        MontyNode::Ellipsis => Kind::Ellipsis(oracle::Unit {}),
        MontyNode::NotImplemented => Kind::NotImplemented(oracle::Unit {}),
        MontyNode::None => Kind::None(oracle::Unit {}),
        MontyNode::Bool(b) => Kind::Boolean(*b),
        MontyNode::Int(i) => Kind::Int(*i),
        MontyNode::BigInt(bi) => {
            let (sign, magnitude) = bi.to_bytes_be();
            Kind::Bigint(oracle::BigInt {
                negative: sign == Sign::Minus,
                magnitude,
            })
        }
        MontyNode::Float(f) => Kind::Float(*f),
        MontyNode::String(s) => Kind::Str(s.clone()),
        MontyNode::Bytes(b) => Kind::Bytes(b.clone()),
        MontyNode::List(ids) => Kind::List(oracle_indexes(ids)),
        MontyNode::Tuple(ids) => Kind::Tuple(oracle_indexes(ids)),
        MontyNode::NamedTuple {
            type_name,
            field_names,
            values,
        } => Kind::NamedTuple(oracle::NamedTupleNode {
            type_name: type_name.clone(),
            field_names: field_names.clone(),
            values: values.iter().map(|id| id.0).collect(),
        }),
        MontyNode::Dict(pairs) => Kind::Dict(oracle_pairs(pairs)),
        MontyNode::Set(ids) => Kind::Set(oracle_indexes(ids)),
        MontyNode::FrozenSet(ids) => Kind::FrozenSet(oracle_indexes(ids)),
        MontyNode::Date(d) => Kind::Date(oracle::Date {
            year: d.year,
            month: u32::from(d.month),
            day: u32::from(d.day),
        }),
        MontyNode::DateTime(dt) => Kind::Datetime(oracle::DateTime {
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
        MontyNode::Time(t) => Kind::Time(oracle::Time {
            hour: u32::from(t.hour),
            minute: u32::from(t.minute),
            second: u32::from(t.second),
            microsecond: t.microsecond,
            offset_seconds: t.offset_seconds,
            timezone_name: t.timezone_name.clone(),
            fold: u32::from(t.fold),
        }),
        MontyNode::TimeDelta(td) => Kind::Timedelta(oracle::TimeDelta {
            days: td.days,
            seconds: td.seconds,
            microseconds: td.microseconds,
        }),
        MontyNode::TimeZone(tz) => Kind::Timezone(oracle::TimeZone {
            offset_seconds: tz.offset_seconds,
            name: tz.name.clone(),
        }),
        MontyNode::Exception { exc_type, arg } => Kind::Exception(oracle::Exception {
            exc_type: exc_type.to_string(),
            arg: arg.clone(),
        }),
        MontyNode::Type(t) => Kind::Type(oracle::Type {
            name: t.to_string(),
            origin: oracle::TypeOrigin::Builtin as i32,
            ..oracle::Type::default()
        }),
        MontyNode::ClassType(class) => Kind::Type(oracle_class_type(class)),
        MontyNode::BuiltinFunction(bf) => Kind::BuiltinFunction(bf.to_string()),
        MontyNode::Path(p) => Kind::Path(p.clone()),
        MontyNode::FileHandle(fh) => Kind::FileHandle(oracle::FileHandle {
            path: fh.path.clone(),
            mode: fh.mode.as_str().to_owned(),
            position: fh.position,
        }),
        MontyNode::ClassInstance {
            class_type,
            instance_id,
            attrs,
        } => Kind::ClassInstance(oracle::ClassInstanceNode {
            class_type: class_type.0,
            instance_id: Some(oracle_uuid(instance_id)),
            attrs: Some(oracle_pairs(attrs)),
        }),
        MontyNode::Function { name, docstring } => Kind::Function(oracle::Function {
            name: name.clone(),
            docstring: docstring.clone(),
        }),
        MontyNode::Repr(r) => Kind::Repr(r.clone()),
        MontyNode::Cycle(placeholder) => Kind::Cycle(placeholder.clone()),
    };
    oracle::MontyNode { kind: Some(kind) }
}

/// Oracle mirror of a class node.
fn oracle_class_type(class: &ClassTypeNode) -> oracle::Type {
    let origin = if class.host_defined {
        oracle::TypeOrigin::Host
    } else {
        oracle::TypeOrigin::Sandbox
    };
    let attrs = if class.attrs.is_empty() {
        None
    } else {
        Some(oracle_pairs(&class.attrs))
    };
    oracle::Type {
        name: class.name.clone(),
        id: Some(oracle_uuid(&class.id)),
        origin: origin as i32,
        is_dataclass: class.is_dataclass,
        attrs,
    }
}

/// Oracle mirror of a `MontyUuid`.
fn oracle_uuid(uuid: &MontyUuid) -> oracle::Uuid {
    oracle::Uuid {
        data: uuid.as_bytes().to_vec(),
    }
}

fn oracle_indexes(ids: &[NodeId]) -> oracle::Indexes {
    oracle::Indexes {
        items: ids.iter().map(|id| id.0).collect(),
    }
}

fn oracle_pairs(pairs: &[(NodeId, NodeId)]) -> oracle::NodePairs {
    oracle::NodePairs {
        pairs: pairs
            .iter()
            .map(|(key, value)| oracle::NodePair {
                key: key.0,
                value: value.0,
            })
            .collect(),
    }
}

/// Decodes wire bytes through the hand-written codec.
fn decode_wire(bytes: &[u8]) -> Result<MontyGraph, String> {
    let wire = WireArena::decode(bytes).map_err(|err| err.to_string())?;
    wire.into_graph().map_err(|err| err.to_string())
}

// ============================================================================
// Byte compatibility on valid values
// ============================================================================

#[test]
fn hand_encoding_matches_generated_encoding() {
    for graph in graphs() {
        let hand = WireArena::new(graph.clone()).encode_to_vec();
        let generated = to_oracle(&graph).encode_to_vec();
        assert_eq!(hand, generated, "encodings diverge for {graph:?}");
    }
}

#[test]
fn hand_decoder_reads_generated_bytes() {
    for graph in graphs() {
        let generated = to_oracle(&graph).encode_to_vec();
        let back = decode_wire(&generated).expect("decode failed");
        assert_eq!(back, graph, "decoding generated bytes diverges for {graph:?}");
    }
}

#[test]
fn generated_decoder_reads_hand_bytes() {
    for graph in graphs() {
        let hand = WireArena::new(graph.clone()).encode_to_vec();
        let back = oracle::Arena::decode(hand.as_slice()).expect("oracle decode failed");
        // compare re-encoded bytes rather than structs: the oracle's derived
        // PartialEq uses IEEE float semantics, under which NaN != NaN
        assert_eq!(
            back.encode_to_vec(),
            hand,
            "oracle decoding hand bytes diverges for {graph:?}"
        );
    }
}

#[test]
fn hand_call_payloads_match_generated_encoding() {
    let args = vec![
        MontyObject::int(1),
        MontyObject::string("arg".to_owned()),
        MontyObject::list([MontyObject::none()]),
    ];
    let kwargs = vec![
        (MontyObject::string("flag".to_owned()), MontyObject::bool(true)),
        (MontyObject::string("count".to_owned()), MontyObject::int(3)),
    ];

    let call = CallArgs::from((args, kwargs));

    // Both receiver states: a routed call (method / `__call__`) and a plain
    // external call (absent field).
    let receivers = [Some(MontyUuid::from_u128(7)), None];
    for (object_id, allow_eager_await) in receivers.into_iter().flat_map(|id| [(id, false), (id, true)]) {
        let oracle_object_id = object_id.map(|uuid| oracle_uuid(&uuid));
        let hand_call = WireFunctionCall::new("external".to_owned(), call.clone(), 42, object_id, allow_eager_await);
        let generated_call = oracle::FunctionCall {
            function_name: "external".to_owned(),
            args: call.args.iter().map(|id| id.0).collect(),
            kwargs: oracle_pairs(&call.kwargs).pairs,
            call_id: 42,
            object_id: oracle_object_id,
            allow_eager_await,
            values: Some(to_oracle(&call.values)),
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

    // `OsCall` is fully generated, but its arena is the hand-written
    // `WireArena` — check the embedding agrees with the oracle byte-for-byte.
    let default = MontyObject::list([MontyObject::none(), MontyObject::int(3)]);
    let hand_os = os_call_to_proto(
        7,
        OsFunctionCall::Getenv(GetenvArgs {
            key: "HOME".to_owned(),
            default: default.clone(),
        }),
    );
    let generated_os = oracle::OsCall {
        call_id: 7,
        values: Some(to_oracle(&default.graph)),
        call: Some(oracle::os_call::Call::Getenv(oracle::os_call::Getenv {
            key: "HOME".to_owned(),
            default: default.root.0,
        })),
    };
    assert_eq!(hand_os.encode_to_vec(), generated_os.encode_to_vec());
    assert_eq!(
        pb::OsCall::decode(generated_os.encode_to_vec().as_slice()).expect("generated os call decodes"),
        hand_os
    );

    // `DateTimeNow` is fully typed (optional TimeZone) — no arena, but keep
    // the byte-compat check against the oracle.
    let hand_now = pb::OsCall {
        call_id: 9,
        values: None,
        call: Some(pb::os_call::Call::DateTimeNow(pb::os_call::DateTimeNow {
            tz: Some(pb::TimeZone {
                offset_seconds: 3600,
                name: Some("CET".to_owned()),
            }),
        })),
    };
    let generated_now = oracle::OsCall {
        call_id: 9,
        values: None,
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

/// A cyclic, shared value exported by real execution must agree byte-for-byte
/// too — it exercises the `Cycle` leaf and node sharing end to end.
#[test]
fn executed_cycle_value_is_byte_compatible() {
    let run = MontyRun::new(
        "a = []\na.append(a)\n[a, a]".to_owned(),
        "test.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap();
    let RunProgress::Complete(value) = run
        .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
        .unwrap()
    else {
        panic!("expected completion");
    };
    // the cycle leaf, `a`, and the outer list: nothing is exported twice
    assert_eq!(value.graph.len(), 3);
    let hand = WireArena::new(value.graph.clone()).encode_to_vec();
    assert_eq!(hand, to_oracle(&value.graph).encode_to_vec());
    assert_eq!(decode_wire(&hand).expect("decode failed"), value.graph);
}

// ============================================================================
// Hostile frames: semantic validation happens during decode
// ============================================================================

/// Encodes an oracle `kind` arm as a one-node arena and decodes it through the
/// hand-written codec, returning the decode error message.
fn rejected(kind: Kind) -> String {
    rejected_nodes(vec![oracle::MontyNode { kind: Some(kind) }])
}

/// Encodes an oracle arena and decodes it through the hand-written codec,
/// returning the decode error message.
fn rejected_nodes(nodes: Vec<oracle::MontyNode>) -> String {
    let bytes = oracle::Arena {
        node_count: u32::try_from(nodes.len()).unwrap(),
        nodes,
    }
    .encode_to_vec();
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
            attrs: Some(oracle::NodePairs::default()),
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
    // a class instance's class must be a class node, not a builtin type leaf
    let builtin_int = oracle::MontyNode {
        kind: Some(Kind::Type(oracle::Type {
            name: "int".to_owned(),
            origin: oracle::TypeOrigin::Builtin as i32,
            ..oracle::Type::default()
        })),
    };
    let instance_of =
        |class_type: u32, instance_id: Option<oracle::Uuid>, attrs: Option<oracle::NodePairs>| oracle::MontyNode {
            kind: Some(Kind::ClassInstance(oracle::ClassInstanceNode {
                class_type,
                instance_id,
                attrs,
            })),
        };
    assert_eq!(
        rejected_nodes(vec![
            builtin_int,
            instance_of(
                0,
                Some(oracle::Uuid { data: vec![0; 16] }),
                Some(oracle::NodePairs::default())
            ),
        ]),
        "invalid value for Arena: class instance node 1 does not point at a class type"
    );
    // instance_id is required
    assert_eq!(
        rejected_nodes(vec![
            oracle::MontyNode {
                kind: Some(Kind::Type(class_type("Foo")))
            },
            instance_of(0, None, Some(oracle::NodePairs::default())),
        ]),
        "failed to decode Protobuf message: missing required field ClassInstanceNode.instance_id"
    );
    // attrs is required, even when empty
    assert_eq!(
        rejected_nodes(vec![
            oracle::MontyNode {
                kind: Some(Kind::Type(class_type("Foo")))
            },
            instance_of(0, Some(oracle::Uuid { data: vec![7; 16] }), None),
        ]),
        "failed to decode Protobuf message: missing required field ClassInstanceNode.attrs"
    );
    // the instance uuid must be exactly 16 bytes
    assert_eq!(
        rejected_nodes(vec![
            oracle::MontyNode {
                kind: Some(Kind::Type(class_type("Foo")))
            },
            instance_of(
                0,
                Some(oracle::Uuid { data: vec![7; 17] }),
                Some(oracle::NodePairs::default())
            ),
        ]),
        "failed to decode Protobuf message: invalid value for ClassInstanceNode.instance_id: uuid must be 16 bytes, got 17"
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
        "failed to decode Protobuf message: missing required field MontyNode.kind"
    );
    // an absent kind is a valid empty message but not a node
    assert_eq!(
        rejected_nodes(vec![oracle::MontyNode { kind: None }]),
        "failed to decode Protobuf message: missing required field MontyNode.kind"
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
    let expected = MontyNode::ClassType(Box::new(ClassTypeNode {
        name: "Foo".to_owned(),
        id: MontyUuid::from_u128(0x0707_0707_0707_0707_0707_0707_0707_0707),
        host_defined: false,
        is_dataclass: false,
        attrs: vec![],
    }));
    let with_empty_attrs = oracle::Type {
        attrs: Some(oracle::NodePairs::default()),
        ..class_type("Foo")
    };
    let bytes = oracle::Arena {
        node_count: 2,
        nodes: vec![
            oracle::MontyNode {
                kind: Some(Kind::Type(with_empty_attrs)),
            },
            oracle::MontyNode {
                kind: Some(Kind::ClassInstance(oracle::ClassInstanceNode {
                    class_type: 0,
                    instance_id: Some(oracle::Uuid { data: vec![7; 16] }),
                    attrs: Some(oracle::NodePairs::default()),
                })),
            },
        ],
    }
    .encode_to_vec();
    assert_eq!(
        decode_wire(&bytes).unwrap().nodes(),
        &[
            expected,
            MontyNode::ClassInstance {
                class_type: NodeId(0),
                instance_id: MontyUuid::from_u128(0x0707_0707_0707_0707_0707_0707_0707_0707),
                attrs: vec![],
            },
        ]
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
    let graph = MontyObject::int(42).graph;
    let mut bytes = WireArena::new(graph.clone()).encode_to_vec();
    // append an unknown varint field: key = 99 << 3 | 0 = 792 (varint
    // 0x98 0x06), value 7
    bytes.extend_from_slice(&[0x98, 0x06, 0x07]);
    assert_eq!(decode_wire(&bytes).expect("unknown field must be skipped"), graph);
}

/// Truncated and corrupt frames never panic: a cut inside a node fails to
/// decode, and a cut between nodes yields a strict prefix of the arena (the
/// carrying message's root ids then catch the missing nodes).
#[test]
fn corrupt_frames_fail_cleanly() {
    let graph = MontyObject::list([MontyObject::int(1)]).graph;
    let bytes = WireArena::new(graph.clone()).encode_to_vec();
    for cut in 1..bytes.len() {
        if let Ok(prefix) = decode_wire(&bytes[..cut]) {
            assert!(prefix.len() < graph.len(), "truncation at {cut} must lose nodes");
            assert_eq!(prefix.nodes(), &graph.nodes()[..prefix.len()]);
        }
    }
}
