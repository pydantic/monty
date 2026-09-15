//! Tests for the value arena: tree ↔ arena round trips, sharing, cycles,
//! validation, merging, expansion limits and footprint accounting.

use std::mem::size_of;

use monty_types::{
    BuiltinsFunctions, CallArgs, DictPairs, ExcType, ExpandError, ExpandLimits, FileMode, GraphError,
    MontyClassInstance, MontyClassType, MontyDate, MontyDateTime, MontyFileHandle, MontyGraph, MontyNode, MontyObject,
    MontyTime, MontyTimeDelta, MontyTimeZone, MontyType, MontyUuid, MontyValue, NamedValues, NodeId,
};

fn class_type(attrs: DictPairs) -> MontyClassType {
    MontyClassType {
        name: "Point".to_owned(),
        id: MontyUuid::from_u128(1),
        host_defined: true,
        is_dataclass: false,
        attrs,
    }
}

fn pair(name: &str, value: MontyObject) -> (MontyObject, MontyObject) {
    (MontyObject::String(name.to_owned()), value)
}

/// One object of every `MontyObject` variant.
fn corpus() -> Vec<MontyObject> {
    vec![
        MontyObject::Ellipsis,
        MontyObject::NotImplemented,
        MontyObject::None,
        MontyObject::Bool(true),
        MontyObject::Int(-42),
        MontyObject::BigInt("123456789012345678901234567890".parse().unwrap()),
        MontyObject::Float(f64::NAN),
        MontyObject::String("hi".to_owned()),
        MontyObject::Bytes(vec![0, 255]),
        MontyObject::List(vec![MontyObject::Int(1), MontyObject::None]),
        MontyObject::Tuple(vec![MontyObject::Int(1)]),
        MontyObject::Set(vec![MontyObject::Int(1)]),
        MontyObject::FrozenSet(vec![MontyObject::Int(1)]),
        MontyObject::NamedTuple {
            type_name: "os.stat_result".to_owned(),
            field_names: vec!["st_mode".to_owned()],
            values: vec![MontyObject::Int(0o644)],
        },
        MontyObject::Dict(DictPairs::from(vec![pair("a", MontyObject::Int(1))])),
        MontyObject::Date(MontyDate {
            year: 2026,
            month: 9,
            day: 15,
        }),
        MontyObject::DateTime(MontyDateTime {
            year: 2026,
            month: 9,
            day: 15,
            hour: 1,
            minute: 2,
            second: 3,
            microsecond: 4,
            offset_seconds: Some(3600),
            timezone_name: Some("CET".to_owned()),
        }),
        MontyObject::Time(MontyTime {
            hour: 1,
            minute: 2,
            second: 3,
            microsecond: 4,
            offset_seconds: None,
            timezone_name: None,
            fold: 0,
        }),
        MontyObject::TimeDelta(MontyTimeDelta {
            days: 1,
            seconds: 2,
            microseconds: 3,
        }),
        MontyObject::TimeZone(MontyTimeZone {
            offset_seconds: 0,
            name: Some("UTC".to_owned()),
        }),
        MontyObject::Exception {
            exc_type: ExcType::ValueError,
            arg: Some("bad".to_owned()),
        },
        MontyObject::Type(MontyType::Int),
        MontyObject::Type(MontyType::Instance(Box::new(class_type(DictPairs::from(vec![pair(
            "ORIGIN",
            MontyObject::Int(0),
        )]))))),
        MontyObject::BuiltinFunction(BuiltinsFunctions::Len),
        MontyObject::Path("/mnt/data".to_owned()),
        MontyObject::FileHandle(MontyFileHandle {
            path: "/mnt/f".to_owned(),
            mode: FileMode::Read(false),
            position: 7,
        }),
        MontyObject::ClassInstance(Box::new(MontyClassInstance {
            class_type: class_type(DictPairs::default()),
            instance_id: MontyUuid::from_u128(2),
            attrs: DictPairs::from(vec![pair("x", MontyObject::Int(1))]),
        })),
        MontyObject::Function {
            name: "fetch".to_owned(),
            docstring: Some("doc".to_owned()),
        },
        MontyObject::Repr("<object>".to_owned()),
        MontyObject::Cycle("[...]".to_owned()),
    ]
}

// === tree → arena → tree ===

#[test]
fn every_variant_round_trips_through_the_arena() {
    for object in corpus() {
        let value = MontyValue::from(object.clone());
        assert_eq!(value.into_object().unwrap(), object, "{object:?}");
    }
}

#[test]
fn class_types_become_their_own_node() {
    let object = MontyObject::ClassInstance(Box::new(MontyClassInstance {
        class_type: class_type(DictPairs::from(vec![pair("ORIGIN", MontyObject::Int(0))])),
        instance_id: MontyUuid::from_u128(2),
        attrs: DictPairs::default(),
    }));
    let value = MontyValue::from(object);
    // "ORIGIN", 0, the class, then the instance.
    assert_eq!(value.graph.len(), 4);
    assert!(matches!(value.graph.node(NodeId(2)), MontyNode::ClassType(class) if class.name == "Point"));
    assert!(matches!(
        value.root_node(),
        MontyNode::ClassInstance {
            class_type: NodeId(2),
            ..
        }
    ));
}

#[test]
fn shared_nodes_expand_into_copies() {
    let mut graph = MontyGraph::new();
    let one = graph.push(MontyNode::Int(1));
    let inner = graph.push(MontyNode::List(vec![one]));
    let root = graph.push(MontyNode::List(vec![inner, inner]));
    let value = MontyValue::new(graph, root).unwrap();
    assert_eq!(
        value.into_object().unwrap(),
        MontyObject::List(vec![
            MontyObject::List(vec![MontyObject::Int(1)]),
            MontyObject::List(vec![MontyObject::Int(1)]),
        ])
    );
    assert_eq!(value.to_string(), "[[1], [1]]");
}

#[test]
fn cycle_leaf_expands_to_the_placeholder() {
    let mut graph = MontyGraph::new();
    let cycle = graph.push(MontyNode::Cycle("[...]".to_owned()));
    let root = graph.push(MontyNode::List(vec![cycle]));
    let value = MontyValue::new(graph, root).unwrap();
    assert_eq!(
        value.into_object().unwrap(),
        MontyObject::List(vec![MontyObject::Cycle("[...]".to_owned())])
    );
    assert_eq!(value.to_string(), "[[...]]");
}

#[test]
fn cycle_placeholders_follow_the_container_kind() {
    assert_eq!(MontyNode::List(vec![]).cycle_placeholder(), "[...]");
    assert_eq!(MontyNode::Tuple(vec![]).cycle_placeholder(), "(...)");
    assert_eq!(MontyNode::Dict(vec![]).cycle_placeholder(), "{...}");
    assert_eq!(MontyNode::Int(1).cycle_placeholder(), "...");
}

// === expansion limits ===

/// `x = [0]; x = [x, x]` repeated: n doublings share 2 + n nodes but expand to 2^(n+1) - 1.
fn doubling_ladder(doublings: u32) -> MontyValue {
    let mut graph = MontyGraph::new();
    let zero = graph.push(MontyNode::Int(0));
    let mut x = graph.push(MontyNode::List(vec![zero]));
    for _ in 0..doublings {
        x = graph.push(MontyNode::List(vec![x, x]));
    }
    MontyValue::new(graph, x).unwrap()
}

#[test]
fn expansion_is_capped_by_bytes() {
    let value = doubling_ladder(20);
    assert_eq!(value.graph.len(), 22);
    let limits = ExpandLimits {
        max_bytes: 1 << 20,
        ..ExpandLimits::default()
    };
    assert_eq!(
        value.into_object_with(limits),
        Err(ExpandError::TooLarge { limit: 1 << 20 })
    );
    assert!(doubling_ladder(3).into_object().is_ok());
}

#[test]
fn expansion_is_capped_by_depth() {
    let mut graph = MontyGraph::new();
    let mut id = graph.push(MontyNode::Int(0));
    for _ in 0..201 {
        id = graph.push(MontyNode::List(vec![id]));
    }
    let value = MontyValue::new(graph, id).unwrap();
    assert_eq!(value.into_object(), Err(ExpandError::TooDeep { limit: 200 }));
    assert!(value.to_string().starts_with("<value not shown: "));
}

// === validation ===

#[test]
fn nodes_must_reference_lower_indexes() {
    let nodes = vec![MontyNode::List(vec![NodeId(1)]), MontyNode::Int(1)];
    assert_eq!(
        MontyGraph::from_nodes(nodes).unwrap_err(),
        GraphError::IndexNotLower {
            node: NodeId(0),
            child: NodeId(1)
        }
    );
    let nodes = vec![MontyNode::List(vec![NodeId(0)])];
    assert!(matches!(
        MontyGraph::from_nodes(nodes),
        Err(GraphError::IndexNotLower { .. })
    ));
}

#[test]
fn class_instances_must_point_at_a_class_type() {
    let nodes = vec![
        MontyNode::Int(1),
        MontyNode::ClassInstance {
            class_type: NodeId(0),
            instance_id: MontyUuid::from_u128(2),
            attrs: vec![],
        },
    ];
    assert_eq!(
        MontyGraph::from_nodes(nodes).unwrap_err(),
        GraphError::ClassTypeNotAClass { node: NodeId(1) }
    );
    let nodes = vec![MontyNode::Type(MontyType::Instance(Box::new(class_type(
        DictPairs::default(),
    ))))];
    assert_eq!(
        MontyGraph::from_nodes(nodes).unwrap_err(),
        GraphError::InstanceTypeLeaf { node: NodeId(0) }
    );
}

#[test]
fn roots_must_be_in_range() {
    let graph = MontyValue::from(MontyObject::Int(1)).graph;
    assert_eq!(
        graph.check_root(NodeId(1)),
        Err(GraphError::RootOutOfRange {
            root: NodeId(1),
            len: 1
        })
    );
    assert!(MontyValue::new(graph, NodeId(1)).is_err());
}

#[test]
#[should_panic(expected = "invalid node pushed onto a MontyGraph")]
fn push_rejects_forward_references() {
    MontyGraph::new().push(MontyNode::List(vec![NodeId(0)]));
}

// === merging and carriers ===

#[test]
fn merge_rebases_ids() {
    let mut target = MontyValue::from(MontyObject::Int(1)).graph;
    let other = MontyValue::from(MontyObject::List(vec![MontyObject::Int(2)]));
    let offset = target.merge(other.graph.clone());
    assert_eq!(offset, 1);
    let root = NodeId(other.root.0 + offset);
    assert_eq!(target.node(root), &MontyNode::List(vec![NodeId(1)]));
    assert_eq!(
        target.value(root).into_object().unwrap(),
        MontyObject::List(vec![MontyObject::Int(2)])
    );
}

#[test]
fn call_args_share_one_arena() {
    let mut call = CallArgs::new();
    let shared = MontyValue::from(MontyObject::List(vec![MontyObject::Int(1)]));
    let first = call.push_arg(shared.clone());
    let second = call.push_arg(shared.as_ref());
    call.push_kwarg("flag", true);
    assert_ne!(first, second);
    assert_eq!(call.values.len(), 6);
    assert!(call.check_roots().is_ok());
    let (args, kwargs) = call.into_objects().unwrap();
    assert_eq!(args.len(), 2);
    assert_eq!(kwargs, vec![pair("flag", MontyObject::Bool(true))]);
    assert_eq!(call.kwargs().next().unwrap().1.type_name(), "bool");
}

#[test]
fn value_ref_copy_preserves_sharing() {
    let ladder = doubling_ladder(3);
    let copy = ladder.as_ref().to_owned();
    assert_eq!(copy.graph.len(), ladder.graph.len());
    assert_eq!(copy, ladder);
}

#[test]
fn named_values_convert_from_pairs() {
    let named = NamedValues::from(vec![
        ("a".to_owned(), MontyObject::Int(1)),
        ("b".to_owned(), MontyObject::String("s".to_owned())),
    ]);
    let names: Vec<_> = named.iter().map(|(name, value)| (name, value.type_name())).collect();
    assert_eq!(names, vec![("a", "int"), ("b", "str")]);
    assert!(named.check_roots().is_ok());
    let mut broken = named.clone();
    broken.names.push(("c".to_owned(), NodeId(9)));
    assert!(broken.check_roots().is_err());
}

// === footprint ===

#[test]
fn node_is_no_wider_than_an_object() {
    assert_eq!(size_of::<MontyNode>(), size_of::<MontyObject>());
}

#[test]
fn host_size_sums_nodes() {
    let value = MontyValue::from(MontyObject::List(vec![MontyObject::String("abc".to_owned())]));
    let list = MontyNode::List(vec![NodeId(0)]);
    assert_eq!(list.host_size(), size_of::<MontyNode>() + size_of::<NodeId>());
    assert_eq!(value.graph.host_size(), size_of::<MontyNode>() + 3 + list.host_size());
}

// === serialization ===

#[test]
fn arena_round_trips_through_serde() {
    let value = doubling_ladder(2);
    let json = serde_json::to_string(&value).unwrap();
    assert_eq!(serde_json::from_str::<MontyValue>(&json).unwrap(), value);
    let bytes = postcard::to_allocvec(&value).unwrap();
    assert_eq!(postcard::from_bytes::<MontyValue>(&bytes).unwrap(), value);
}
