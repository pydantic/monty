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

use std::time::Instant;

use monty::{MontyRun, RunProgress};
use monty_proto::{WireArena, WireFunctionCall, decode_frame, os_call_to_proto, pb};
use monty_types::{
    CallArgs, CompileOptions, ExcType, GetenvArgs, MontyDate, MontyDateTime, MontyFileHandle, MontyObject, MontyTime,
    MontyTimeDelta, MontyTimeZone, MontyType, MontyUuid, OsFunctionCall, PrintWriter, ResourceTracker, SourceRange,
    unstable::{self, ClassTypeNode, MontyGraph, MontyNode, NodeId},
};
use num_bigint::{BigInt, Sign};
use prost::{
    Message,
    encoding::{WireType, encode_key, encode_varint},
};

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
        MontyObject::complex(0.0, 0.0), // both parts present even at the default payload
        MontyObject::complex(-0.0, -0.0),
        MontyObject::complex(1.5, -2.0),
        MontyObject::complex(f64::NAN, f64::NEG_INFINITY),
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
    let mut graphs: Vec<MontyGraph> = corpus()
        .into_iter()
        .map(|value| unstable::into_graph_parts(value).0)
        .collect();
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
        MontyNode::Complex(c) => Kind::Complex(oracle::Complex {
            real: Some(c.real),
            imag: Some(c.imag),
        }),
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
    let wire = decode_frame::<WireArena>(bytes).map_err(|err| err.to_string())?;
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

/// Extern-mapped reference buffers preserve generated node encoding and length calculations.
#[test]
fn reference_adapters_match_generated_encoding() {
    for graph in graphs() {
        for node in to_oracle(&graph).nodes {
            let bytes = node.encode_to_vec();
            let decoded = decode_frame::<pb::MontyNode>(&bytes).unwrap();
            assert_eq!(decoded.encoded_len(), bytes.len());
            assert_eq!(decoded.encode_to_vec(), bytes);
        }
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
    let (graph, arg_ids, kwarg_ids) = unstable::call_args_parts(&call);

    // Both receiver states: a routed call (method / `__call__`) and a plain
    // external call (absent field).
    let receivers = [Some(MontyUuid::from_u128(7)), None];
    for (object_id, allow_eager_await) in receivers.into_iter().flat_map(|id| [(id, false), (id, true)]) {
        let oracle_object_id = object_id.map(|uuid| oracle_uuid(&uuid));
        let hand_call = WireFunctionCall::new(
            "external".to_owned(),
            call.clone(),
            42,
            object_id,
            allow_eager_await,
            position(),
        );
        let generated_call = oracle::FunctionCall {
            function_name: "external".to_owned(),
            args: arg_ids.iter().map(|id| id.0).collect(),
            kwargs: oracle_pairs(kwarg_ids).pairs,
            call_id: 42,
            object_id: oracle_object_id,
            allow_eager_await,
            values: Some(to_oracle(graph)),
            position: Some(oracle_position()),
        };
        assert_eq!(hand_call.encode_to_vec(), generated_call.encode_to_vec());
        assert_eq!(
            decode_frame::<WireFunctionCall>(generated_call.encode_to_vec().as_slice())
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
        false,
        &position(),
    );
    let (graph, root) = unstable::graph_parts(&default);
    let generated_os = oracle::OsCall {
        call_id: 7,
        values: Some(to_oracle(graph)),
        allow_eager_await: false,
        call: Some(oracle::os_call::Call::Getenv(oracle::os_call::Getenv {
            key: "HOME".to_owned(),
            default: root.0,
        })),
        position: Some(oracle_position()),
    };
    assert_eq!(hand_os.encode_to_vec(), generated_os.encode_to_vec());
    assert_eq!(
        decode_frame::<pb::OsCall>(generated_os.encode_to_vec().as_slice()).expect("generated os call decodes"),
        hand_os
    );

    // `DateTimeNow` is fully typed (optional TimeZone) — no arena, but keep
    // the byte-compat check against the oracle.
    let hand_now = pb::OsCall {
        call_id: 9,
        values: None,
        allow_eager_await: false,
        call: Some(pb::os_call::Call::DateTimeNow(pb::os_call::DateTimeNow {
            tz: Some(pb::TimeZone {
                offset_seconds: 3600,
                name: Some("CET".to_owned()),
            }),
        })),
        position: Some((&position()).into()),
    };
    let generated_now = oracle::OsCall {
        call_id: 9,
        values: None,
        allow_eager_await: false,
        call: Some(oracle::os_call::Call::DateTimeNow(oracle::os_call::DateTimeNow {
            tz: Some(oracle::TimeZone {
                offset_seconds: 3600,
                name: Some("CET".to_owned()),
            }),
        })),
        position: Some(oracle_position()),
    };
    assert_eq!(hand_now.encode_to_vec(), generated_now.encode_to_vec());
    assert_eq!(
        decode_frame::<pb::OsCall>(generated_now.encode_to_vec().as_slice()).expect("generated now call decodes"),
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
    let (graph, _) = unstable::graph_parts(&value);
    assert_eq!(graph.len(), 3);
    let hand = WireArena::new(graph.clone()).encode_to_vec();
    assert_eq!(hand, to_oracle(graph).encode_to_vec());
    assert_eq!(&decode_wire(&hand).expect("decode failed"), graph);
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
        "frame decode error: failed to decode Protobuf message: unknown exception type \"NotARealError\""
    );
    assert_eq!(
        rejected(Kind::Type(oracle::Type {
            name: "NotAType".to_owned(),
            origin: oracle::TypeOrigin::Builtin as i32,
            ..oracle::Type::default()
        })),
        "frame decode error: failed to decode Protobuf message: unknown type name \"NotAType\""
    );
    // origin must be specified
    assert_eq!(
        rejected(Kind::Type(oracle::Type {
            name: "int".to_owned(),
            ..oracle::Type::default()
        })),
        "frame decode error: failed to decode Protobuf message: invalid value for Type: origin must be specified"
    );
    // a builtin must not carry an id
    assert_eq!(
        rejected(Kind::Type(oracle::Type {
            name: "int".to_owned(),
            origin: oracle::TypeOrigin::Builtin as i32,
            id: Some(oracle::Uuid { data: vec![0; 16] }),
            ..oracle::Type::default()
        })),
        "frame decode error: failed to decode Protobuf message: invalid value for Type: a builtin type must not carry an id"
    );
    assert_eq!(
        rejected(Kind::Type(oracle::Type {
            name: "int".to_owned(),
            origin: oracle::TypeOrigin::Builtin as i32,
            attrs: Some(oracle::NodePairs::default()),
            ..oracle::Type::default()
        })),
        "frame decode error: failed to decode Protobuf message: invalid value for Type: a builtin type must not carry attrs"
    );
    // a class type must carry an id
    assert_eq!(
        rejected(Kind::Type(oracle::Type {
            name: "Foo".to_owned(),
            origin: oracle::TypeOrigin::Host as i32,
            ..oracle::Type::default()
        })),
        "frame decode error: failed to decode Protobuf message: invalid value for Type: a class type must carry an id"
    );
    // a uuid must be exactly 16 bytes
    assert_eq!(
        rejected(Kind::Type(oracle::Type {
            name: "Foo".to_owned(),
            origin: oracle::TypeOrigin::Sandbox as i32,
            id: Some(oracle::Uuid { data: vec![0; 5] }),
            ..oracle::Type::default()
        })),
        "frame decode error: failed to decode Protobuf message: invalid value for Type.id: uuid must be 16 bytes, got 5"
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
        "frame decode error: failed to decode Protobuf message: missing required field ClassInstanceNode.instance_id"
    );
    // attrs is required, even when empty
    assert_eq!(
        rejected_nodes(vec![
            oracle::MontyNode {
                kind: Some(Kind::Type(class_type("Foo")))
            },
            instance_of(0, Some(oracle::Uuid { data: vec![7; 16] }), None),
        ]),
        "frame decode error: failed to decode Protobuf message: missing required field ClassInstanceNode.attrs"
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
        "frame decode error: failed to decode Protobuf message: invalid value for ClassInstanceNode.instance_id: uuid must be 16 bytes, got 17"
    );
    // an origin outside the enum is rejected rather than defaulted
    assert_eq!(
        rejected(Kind::Type(oracle::Type {
            origin: 99,
            ..class_type("Foo")
        })),
        "frame decode error: failed to decode Protobuf message: invalid value for Type.origin: unknown origin 99"
    );
    assert_eq!(
        rejected(Kind::BuiltinFunction("not_a_builtin".to_owned())),
        "frame decode error: failed to decode Protobuf message: unknown builtin function \"not_a_builtin\""
    );
    // update file modes are not yet supported by monty's parser
    assert_eq!(
        rejected(Kind::FileHandle(oracle::FileHandle {
            path: "/f".to_owned(),
            mode: "r+".to_owned(),
            position: 0,
        })),
        "frame decode error: failed to decode Protobuf message: invalid file mode \"r+\""
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
        "frame decode error: failed to decode Protobuf message: invalid value for DateTime.timezone_name: timezone_name requires offset_seconds"
    );
    // the uuid arm is declared in the schema but not yet implemented: the
    // UUID values are reserved by the schema but rejected by domain conversion
    assert_eq!(
        rejected(Kind::Uuid(oracle::Uuid { data: vec![7; 16] })),
        "frame decode error: failed to decode Protobuf message: missing required field MontyNode.kind"
    );
    // an absent kind is a valid empty message but not a node
    assert_eq!(
        rejected_nodes(vec![oracle::MontyNode { kind: None }]),
        "frame decode error: failed to decode Protobuf message: missing required field MontyNode.kind"
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

/// A message field repeated on the wire merges, as prost's generated decoder
/// does: two `Type.attrs` payloads decode to their concatenated pairs on both
/// sides, so a hand-decoded class node never diverges from the oracle.
#[test]
fn repeated_attrs_fields_merge_like_the_oracle() {
    let pair = |key: u32, value: u32| oracle::NodePairs {
        pairs: vec![oracle::NodePair { key, value }],
    };
    // the second `Type` carries only `attrs`: protobuf merges concatenated
    // encodings of one message
    let mut type_body = oracle::Type {
        attrs: Some(pair(0, 1)),
        ..class_type("Foo")
    }
    .encode_to_vec();
    type_body.extend(
        oracle::Type {
            attrs: Some(pair(1, 0)),
            ..oracle::Type::default()
        }
        .encode_to_vec(),
    );
    let mut node = Vec::new();
    encode_key(23, WireType::LengthDelimited, &mut node);
    encode_varint(type_body.len() as u64, &mut node);
    node.extend(type_body);
    let mut bytes = oracle::Arena {
        node_count: 3,
        nodes: vec![
            oracle::MontyNode {
                kind: Some(Kind::Str("a".to_owned())),
            },
            oracle::MontyNode {
                kind: Some(Kind::Int(1)),
            },
        ],
    }
    .encode_to_vec();
    encode_key(2, WireType::LengthDelimited, &mut bytes);
    encode_varint(node.len() as u64, &mut bytes);
    bytes.extend(node);

    let merged = vec![(NodeId(0), NodeId(1)), (NodeId(1), NodeId(0))];
    let graph = decode_wire(&bytes).unwrap();
    let MontyNode::ClassType(class) = &graph.nodes()[2] else {
        panic!("expected a class node");
    };
    assert_eq!(class.attrs, merged);
    let back = oracle::Arena::decode(bytes.as_slice()).expect("oracle decode failed");
    let Some(Kind::Type(ty)) = &back.nodes[2].kind else {
        panic!("expected an oracle type");
    };
    assert_eq!(ty.attrs.as_ref().unwrap().pairs.len(), 2);
}

/// Duplicate message kinds merge before semantic validation, as in the generated oracle.
#[test]
fn repeated_node_kinds_merge_like_the_oracle() {
    let cases = [
        (
            Kind::List(oracle::Indexes { items: vec![0] }),
            Kind::List(oracle::Indexes { items: vec![1] }),
            MontyNode::List(vec![NodeId(0), NodeId(1)]),
        ),
        (
            Kind::Date(oracle::Date {
                year: 2024,
                month: 2,
                day: 0,
            }),
            Kind::Date(oracle::Date {
                year: 0,
                month: 0,
                day: 29,
            }),
            MontyNode::Date(MontyDate {
                year: 2024,
                month: 2,
                day: 29,
            }),
        ),
        (
            Kind::List(oracle::Indexes { items: vec![u32::MAX] }),
            Kind::Int(7),
            MontyNode::Int(7),
        ),
    ];
    for (first, second, expected) in cases {
        let mut node = oracle::MontyNode { kind: Some(first) }.encode_to_vec();
        node.extend(oracle::MontyNode { kind: Some(second) }.encode_to_vec());
        let mut bytes = oracle::Arena {
            node_count: 3,
            nodes: vec![
                oracle::MontyNode {
                    kind: Some(Kind::None(oracle::Unit {})),
                },
                oracle::MontyNode {
                    kind: Some(Kind::Int(1)),
                },
            ],
        }
        .encode_to_vec();
        encode_key(2, WireType::LengthDelimited, &mut bytes);
        encode_varint(node.len() as u64, &mut bytes);
        bytes.extend(node);

        let graph = decode_wire(&bytes).unwrap();
        assert_eq!(graph.nodes()[2], expected);
        assert_eq!(oracle::Arena::decode(bytes.as_slice()).unwrap(), to_oracle(&graph));
    }
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
        "frame decode error: failed to decode Protobuf message: invalid value for Date.month: 4096 is outside the range 1..=12"
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
        "frame decode error: failed to decode Protobuf message: invalid value for TimeZone.offset_seconds: 86400 is outside the range -86399..=86399"
    );
}

// ============================================================================
// Wire-level behaviours
// ============================================================================

/// Unknown fields must be skipped (forward compatibility), exactly like
/// prost's generated decoder.
#[test]
fn unknown_fields_are_skipped() {
    let (graph, _) = unstable::into_graph_parts(MontyObject::int(42));
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
    let (graph, _) = unstable::into_graph_parts(MontyObject::list([MontyObject::int(1)]));
    let bytes = WireArena::new(graph.clone()).encode_to_vec();
    for cut in 1..bytes.len() {
        if let Ok(prefix) = decode_wire(&bytes[..cut]) {
            assert!(prefix.len() < graph.len(), "truncation at {cut} must lose nodes");
            assert_eq!(prefix.nodes(), &graph.nodes()[..prefix.len()]);
        }
    }
}

/// The suspension position every hand-built event carries.
fn position() -> SourceRange {
    SourceRange {
        filename: "main.py".to_owned(),
        start: 30,
        end: 44,
    }
}

/// [`position`] on the oracle's generated types.
fn oracle_position() -> oracle::SourceRange {
    oracle::SourceRange {
        filename: "main.py".to_owned(),
        start: 30,
        end: 44,
    }
}

/// A `FunctionCall.position` split across two tag-8 fields merges into one
/// range on both sides, as protobuf requires of a singular message field.
#[test]
fn repeated_position_fields_merge_like_the_oracle() {
    let args = CallArgs::new();
    let (graph, _, _) = unstable::call_args_parts(&args);
    let mut bytes = oracle::FunctionCall {
        function_name: "external".to_owned(),
        args: vec![],
        kwargs: vec![],
        call_id: 1,
        object_id: None,
        allow_eager_await: false,
        values: Some(to_oracle(graph)),
        position: Some(oracle::SourceRange {
            filename: "main.py".to_owned(),
            start: 30,
            end: 0,
        }),
    }
    .encode_to_vec();
    // the second occurrence carries only the end
    let tail = oracle::SourceRange {
        filename: String::new(),
        start: 0,
        end: 44,
    }
    .encode_to_vec();
    encode_key(8, WireType::LengthDelimited, &mut bytes);
    encode_varint(tail.len() as u64, &mut bytes);
    bytes.extend(tail);

    let hand = decode_frame::<WireFunctionCall>(bytes.as_slice()).expect("split position decodes");
    assert_eq!(hand.position, Some(position()));
    let generated = oracle::FunctionCall::decode(bytes.as_slice()).expect("oracle decodes");
    assert_eq!(generated.position, Some(oracle_position()));

    // many empty repeats after a long filename must cost their own two bytes
    // each, not a copy of the filename per repeat
    let filename = "f".repeat(1 << 20);
    let mut bytes = oracle::FunctionCall {
        function_name: "external".to_owned(),
        args: vec![],
        kwargs: vec![],
        call_id: 1,
        object_id: None,
        allow_eager_await: false,
        values: Some(to_oracle(graph)),
        position: Some(oracle::SourceRange {
            filename: filename.clone(),
            start: 0,
            end: 1,
        }),
    }
    .encode_to_vec();
    for _ in 0..100_000 {
        encode_key(8, WireType::LengthDelimited, &mut bytes);
        encode_varint(0, &mut bytes);
    }
    let started = Instant::now();
    let hand = decode_frame::<WireFunctionCall>(bytes.as_slice()).expect("repeated empty positions decode");
    let expected = filename[..SourceRange::MAX_FILENAME_LEN].to_owned();
    assert_eq!(hand.position.map(|p| p.filename), Some(expected));
    assert!(started.elapsed().as_secs() < 5, "repeats amplified decode work");
}
