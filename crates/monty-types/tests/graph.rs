//! Tests for the value arena: construction, sharing, cycles, validation,
//! merging, equality and footprint accounting.

use std::mem::size_of;

use monty_types::{
    BuiltinsFunctions, CallArgs, ExcType, FileMode, GraphError, MontyDate, MontyDateTime, MontyFileHandle, MontyGraph,
    MontyNode, MontyObject, MontyTime, MontyTimeDelta, MontyTimeZone, MontyType, MontyUuid, NamedValues, NodeId,
};

fn class_type(attrs: impl IntoIterator<Item = (MontyObject, MontyObject)>) -> MontyObject {
    MontyObject::class_type("Point", MontyUuid::from_u128(1), true, false, attrs)
}

fn pair(name: &str, value: MontyObject) -> (MontyObject, MontyObject) {
    (MontyObject::string(name.to_owned()), value)
}

/// One object of every `MontyObject` variant.
fn corpus() -> Vec<MontyObject> {
    vec![
        MontyObject::ellipsis(),
        MontyObject::not_implemented(),
        MontyObject::none(),
        MontyObject::bool(true),
        MontyObject::int(-42),
        MontyObject::bigint("123456789012345678901234567890".parse().unwrap()),
        MontyObject::float(f64::NAN),
        MontyObject::string("hi".to_owned()),
        MontyObject::bytes(vec![0, 255]),
        MontyObject::list([MontyObject::int(1), MontyObject::none()]),
        MontyObject::tuple([MontyObject::int(1)]),
        MontyObject::set([MontyObject::int(1)]),
        MontyObject::frozenset([MontyObject::int(1)]),
        MontyObject::named_tuple(
            "os.stat_result".to_owned(),
            vec!["st_mode".to_owned()],
            vec![MontyObject::int(0o644)],
        ),
        MontyObject::dict([pair("a", MontyObject::int(1))]),
        MontyObject::date(MontyDate {
            year: 2026,
            month: 9,
            day: 15,
        }),
        MontyObject::datetime(MontyDateTime {
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
        MontyObject::time(MontyTime {
            hour: 1,
            minute: 2,
            second: 3,
            microsecond: 4,
            offset_seconds: None,
            timezone_name: None,
            fold: 0,
        }),
        MontyObject::timedelta(MontyTimeDelta {
            days: 1,
            seconds: 2,
            microseconds: 3,
        }),
        MontyObject::timezone(MontyTimeZone {
            offset_seconds: 0,
            name: Some("UTC".to_owned()),
        }),
        MontyObject::exception(ExcType::ValueError, Some("bad".to_owned())),
        MontyObject::type_object(MontyType::Int),
        class_type([pair("ORIGIN", MontyObject::int(0))]),
        MontyObject::builtin_function(BuiltinsFunctions::Len),
        MontyObject::path("/mnt/data".to_owned()),
        MontyObject::file_handle(MontyFileHandle {
            path: "/mnt/f".to_owned(),
            mode: FileMode::Read(false),
            position: 7,
        }),
        MontyObject::class_instance(
            class_type([]),
            MontyUuid::from_u128(2),
            [pair("x", MontyObject::int(1))],
        ),
        MontyObject::function("fetch".to_owned(), Some("doc".to_owned())),
        MontyObject::repr("<object>".to_owned()),
        MontyObject::cycle("[...]".to_owned()),
    ]
}

// === construction ===

#[test]
fn every_kind_copies_and_serializes_equal() {
    for value in corpus() {
        assert_eq!(value.as_ref().to_owned(), value, "{value:?}");
        let bytes = postcard::to_allocvec(&value).unwrap();
        assert_eq!(postcard::from_bytes::<MontyObject>(&bytes).unwrap(), value, "{value:?}");
    }
}

#[test]
fn class_types_become_their_own_node() {
    let value = MontyObject::class_instance(
        class_type([pair("ORIGIN", MontyObject::int(0))]),
        MontyUuid::from_u128(2),
        [],
    );
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
fn shared_nodes_equal_their_copies() {
    let mut graph = MontyGraph::new();
    let one = graph.push(MontyNode::Int(1));
    let inner = graph.push(MontyNode::List(vec![one]));
    let root = graph.push(MontyNode::List(vec![inner, inner]));
    let value = MontyObject::new(graph, root).unwrap();
    assert_eq!(
        value,
        MontyObject::list([
            MontyObject::list([MontyObject::int(1)]),
            MontyObject::list([MontyObject::int(1)]),
        ])
    );
    assert_eq!(value.to_string(), "[[1], [1]]");
}

#[test]
fn cycle_leaf_renders_as_the_placeholder() {
    let mut graph = MontyGraph::new();
    let cycle = graph.push(MontyNode::Cycle("[...]".to_owned()));
    let root = graph.push(MontyNode::List(vec![cycle]));
    let value = MontyObject::new(graph, root).unwrap();
    assert_eq!(value, MontyObject::list([MontyObject::cycle("[...]")]));
    assert_eq!(value.to_string(), "[[...]]");
}

#[test]
fn cycle_placeholders_follow_the_container_kind() {
    assert_eq!(MontyNode::List(vec![]).cycle_placeholder(), "[...]");
    assert_eq!(MontyNode::Tuple(vec![]).cycle_placeholder(), "(...)");
    assert_eq!(MontyNode::Dict(vec![]).cycle_placeholder(), "{...}");
    assert_eq!(MontyNode::Int(1).cycle_placeholder(), "...");
}

// === equality is linear ===

/// `x = [0]; x = [x, x]` repeated: n doublings share 2 + n nodes but would expand to 2^(n+1) - 1.
fn doubling_ladder(doublings: u32) -> MontyObject {
    let mut graph = MontyGraph::new();
    let zero = graph.push(MontyNode::Int(0));
    let mut x = graph.push(MontyNode::List(vec![zero]));
    for _ in 0..doublings {
        x = graph.push(MontyNode::List(vec![x, x]));
    }
    MontyObject::new(graph, x).unwrap()
}

#[test]
fn equality_compares_each_node_pair_once() {
    // 2^61 leaves if expanded: only linear comparison finishes
    let value = doubling_ladder(60);
    assert_eq!(value.graph.len(), 62);
    assert_eq!(value, doubling_ladder(60));
    assert_ne!(value, doubling_ladder(59));
    assert_eq!(
        value.graph.host_size(),
        62 * size_of::<MontyNode>() + 121 * size_of::<NodeId>()
    );
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
}

#[test]
fn roots_must_be_in_range() {
    let graph = MontyObject::int(1).graph;
    assert_eq!(
        graph.check_root(NodeId(1)),
        Err(GraphError::RootOutOfRange {
            root: NodeId(1),
            len: 1
        })
    );
    assert!(MontyObject::new(graph, NodeId(1)).is_err());
}

#[test]
#[should_panic(expected = "invalid node pushed onto a MontyGraph")]
fn push_rejects_forward_references() {
    MontyGraph::new().push(MontyNode::List(vec![NodeId(0)]));
}

// === merging and carriers ===

#[test]
fn merge_rebases_ids() {
    let mut target = MontyObject::int(1).graph;
    let other = MontyObject::list([MontyObject::int(2)]);
    let offset = target.merge(other.graph.clone());
    assert_eq!(offset, 1);
    let root = NodeId(other.root.0 + offset);
    assert_eq!(target.node(root), &MontyNode::List(vec![NodeId(1)]));
    assert_eq!(target.value(root), other);
}

#[test]
fn call_args_share_one_arena() {
    let mut call = CallArgs::new();
    let shared = MontyObject::list([MontyObject::int(1)]);
    let first = call.push_arg(shared.clone());
    let second = call.push_arg(shared.as_ref());
    call.push_kwarg("flag", true);
    assert_ne!(first, second);
    assert_eq!(call.values.len(), 6);
    assert!(call.check_roots().is_ok());
    assert!(call.args().all(|arg| arg == shared));
    assert_eq!(call.kwarg("flag").unwrap(), MontyObject::bool(true));
    assert_eq!(call.kwargs().next().unwrap().1.type_name(), "bool");
    assert_eq!(CallArgs::from(vec![shared.clone()]).arg(0).unwrap(), shared);
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
        ("a".to_owned(), MontyObject::int(1)),
        ("b".to_owned(), MontyObject::string("s".to_owned())),
    ]);
    let names: Vec<_> = named.iter().map(|(name, value)| (name, value.type_name())).collect();
    assert_eq!(names, vec![("a", "int"), ("b", "str")]);
    assert!(named.check_roots().is_ok());
    let mut broken = named.clone();
    broken.names.push(("c".to_owned(), NodeId(9)));
    assert!(broken.check_roots().is_err());
}

// === footprint ===

/// Every arena node costs this much: the widest inline variant is
/// `NamedTuple` (three 24-byte fields), so a larger payload must be boxed
/// rather than inlined (see `ClassType`).
#[test]
fn node_is_72_bytes() {
    assert_eq!(size_of::<MontyNode>(), 72);
}

#[test]
fn host_size_sums_nodes() {
    let value = MontyObject::list([MontyObject::string("abc".to_owned())]);
    let list = MontyNode::List(vec![NodeId(0)]);
    assert_eq!(list.host_size(), size_of::<MontyNode>() + size_of::<NodeId>());
    assert_eq!(value.graph.host_size(), size_of::<MontyNode>() + 3 + list.host_size());
}

// === serialization ===

#[test]
fn arena_round_trips_through_serde() {
    let value = doubling_ladder(2);
    let json = serde_json::to_string(&value).unwrap();
    assert_eq!(serde_json::from_str::<MontyObject>(&json).unwrap(), value);
    let bytes = postcard::to_allocvec(&value).unwrap();
    assert_eq!(postcard::from_bytes::<MontyObject>(&bytes).unwrap(), value);
}
