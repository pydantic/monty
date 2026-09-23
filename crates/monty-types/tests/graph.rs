//! Tests for the value arena: construction, sharing, cycles, validation,
//! merging, equality and footprint accounting.

use std::mem::size_of;

use monty_types::{
    BuiltinsFunctions, CallArgs, ExcType, FileMode, MontyDate, MontyDateTime, MontyFileHandle, MontyObject, MontyTime,
    MontyTimeDelta, MontyTimeZone, MontyType, MontyUuid, NamedValues,
    unstable::{self, GraphError, MontyGraph, MontyNode, NodeId},
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
        let bytes = minicbor_serde::to_vec(&value).unwrap();
        assert_eq!(
            minicbor_serde::from_slice::<MontyObject>(&bytes).unwrap(),
            value,
            "{value:?}"
        );
    }
}

/// Borrowing and consuming the graph must preserve its storage and root pairing.
#[test]
fn unstable_graph_access_preserves_storage() {
    for value in corpus() {
        let (graph, root) = unstable::graph_parts(&value);
        let nodes = graph.nodes().as_ptr();
        let bytes = minicbor_serde::to_vec(&value).unwrap();
        assert_eq!(graph.node(root), unstable::root_node(&value));
        assert_eq!(unstable::node(value.as_ref()), graph.node(root));
        assert_eq!(unstable::child(value.as_ref(), root), value.as_ref());
        assert_eq!(graph.value(root), value.as_ref());
        if graph.node(root).is_leaf() {
            assert_eq!(unstable::object_from_node(graph.node(root).clone()), value);
        }
        let (graph, owned_root) = unstable::into_graph_parts(value);
        assert_eq!(owned_root, root);
        assert_eq!(graph.nodes().as_ptr(), nodes);
        let rebuilt = unstable::object_from_graph(graph, owned_root).unwrap();
        assert_eq!(minicbor_serde::to_vec(&rebuilt).unwrap(), bytes);
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
    let (graph, _) = unstable::graph_parts(&value);
    assert_eq!(graph.len(), 4);
    assert!(matches!(graph.node(NodeId(2)), MontyNode::ClassType(class) if class.name == "Point"));
    assert!(matches!(
        unstable::root_node(&value),
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
    let value = unstable::object_from_graph(graph, root).unwrap();
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
    let value = unstable::object_from_graph(graph, root).unwrap();
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
    unstable::object_from_graph(graph, x).unwrap()
}

#[test]
fn equality_compares_each_node_pair_once() {
    // 2^61 leaves if expanded: only linear comparison finishes
    let value = doubling_ladder(60);
    assert_eq!(unstable::graph_parts(&value).0.len(), 62);
    assert_eq!(value, doubling_ladder(60));
    assert_ne!(value, doubling_ladder(59));
    assert_eq!(
        unstable::graph_parts(&value).0.decoded_size(),
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
    let (graph, _) = unstable::into_graph_parts(MontyObject::int(1));
    assert_eq!(
        graph.check_root(NodeId(1)),
        Err(GraphError::RootOutOfRange {
            root: NodeId(1),
            len: 1
        })
    );
    assert!(unstable::object_from_graph(graph, NodeId(1)).is_err());
}

#[test]
#[should_panic(expected = "invalid node pushed onto a MontyGraph")]
fn push_rejects_forward_references() {
    MontyGraph::new().push(MontyNode::List(vec![NodeId(0)]));
}

// === merging and carriers ===

#[test]
fn merge_rebases_ids() {
    let (mut target, _) = unstable::into_graph_parts(MontyObject::int(1));
    let other = MontyObject::list([MontyObject::int(2)]);
    let (graph, root) = unstable::graph_parts(&other);
    let offset = target.merge(graph.clone());
    assert_eq!(offset, 1);
    let root = NodeId(root.0 + offset);
    assert_eq!(target.node(root), &MontyNode::List(vec![NodeId(1)]));
    assert_eq!(target.value(root), other);
}

#[test]
fn call_args_share_one_arena() {
    let mut call = CallArgs::new();
    let shared = MontyObject::list([MontyObject::int(1)]);
    let first = unstable::push_arg(&mut call, shared.clone());
    let second = unstable::push_arg(&mut call, shared.as_ref());
    call.push_kwarg("flag", MontyObject::bool(true));
    assert_ne!(first, second);
    assert_eq!(unstable::call_args_parts(&call).0.len(), 6);
    assert!(call.args().all(|arg| arg == shared));
    assert_eq!(call.kwarg("flag").unwrap(), MontyObject::bool(true));
    assert_eq!(call.kwargs().next().unwrap().1.type_name(), "bool");
    assert_eq!(CallArgs::from(vec![shared.clone()]).arg(0).unwrap(), shared);
}

#[test]
fn object_ref_copy_preserves_sharing() {
    let ladder = doubling_ladder(3);
    let copy = ladder.as_ref().to_owned();
    assert_eq!(
        unstable::graph_parts(&copy).0.len(),
        unstable::graph_parts(&ladder).0.len()
    );
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
    let (graph, mut names) = unstable::into_named_values_parts(named);
    names.push(("c".to_owned(), NodeId(9)));
    assert_eq!(
        unstable::named_values_from_parts(graph, names),
        Err(GraphError::RootOutOfRange {
            root: NodeId(9),
            len: 2
        })
    );
}

/// Raw call access preserves allocations, sharing and serialized storage.
#[test]
fn unstable_call_parts_preserve_storage() {
    let mut graph = MontyGraph::new();
    let value = graph.push(MontyNode::Int(42));
    let key = graph.push(MontyNode::String("answer".to_owned()));
    let call = unstable::call_args_from_parts(graph, vec![value, value], vec![(key, value)]).unwrap();
    let (graph, args, kwargs) = unstable::call_args_parts(&call);
    let pointers = (graph.nodes().as_ptr(), args.as_ptr(), kwargs.as_ptr());
    let bytes = minicbor_serde::to_vec(&call).unwrap();
    let (graph, args, kwargs) = unstable::into_call_args_parts(call);
    assert_eq!((graph.nodes().as_ptr(), args.as_ptr(), kwargs.as_ptr()), pointers);
    let call = unstable::call_args_from_parts(graph, args, kwargs).unwrap();
    assert_eq!(minicbor_serde::to_vec(&call).unwrap(), bytes);
    let (_, args, kwargs) = unstable::call_args_parts(&call);
    assert_eq!(args, [value, value]);
    assert_eq!(kwargs, [(key, value)]);
    assert_eq!(call.arg(0).unwrap(), MontyObject::int(42));
    assert_eq!(call.kwarg("answer").unwrap(), call.arg(0).unwrap());
}

/// Raw named-value access preserves the arena and the vector of named roots.
#[test]
fn unstable_named_parts_preserve_storage() {
    let mut named = NamedValues::new();
    let id = unstable::push_named(&mut named, "a", 42_i64);
    let (graph, mut names) = unstable::into_named_values_parts(named);
    names.push(("b".to_owned(), id));
    let named = unstable::named_values_from_parts(graph, names).unwrap();
    let (graph, names) = unstable::named_values_parts(&named);
    let pointers = (graph.nodes().as_ptr(), names.as_ptr());
    let bytes = minicbor_serde::to_vec(&named).unwrap();
    let (graph, names) = unstable::into_named_values_parts(named);
    assert_eq!((graph.nodes().as_ptr(), names.as_ptr()), pointers);
    let named = unstable::named_values_from_parts(graph, names).unwrap();
    assert_eq!(minicbor_serde::to_vec(&named).unwrap(), bytes);
    assert_eq!(named.len(), 2);
    assert_eq!(named.iter().len(), 2);
    assert!(named.iter().all(|(_, value)| value.as_int() == Some(42)));
}

/// Every kind of call root is checked, including keyword keys and values.
#[test]
fn unstable_call_construction_rejects_invalid_roots() {
    let graph = MontyGraph::from_nodes(vec![MontyNode::None]).unwrap();
    for (args, kwargs) in [
        (vec![NodeId(1)], vec![]),
        (vec![], vec![(NodeId(1), NodeId(0))]),
        (vec![], vec![(NodeId(0), NodeId(1))]),
    ] {
        assert_eq!(
            unstable::call_args_from_parts(graph.clone(), args, kwargs),
            Err(GraphError::RootOutOfRange {
                root: NodeId(1),
                len: 1
            })
        );
    }
    assert_eq!(
        unstable::call_args_from_parts(MontyGraph::new(), vec![], vec![]).unwrap(),
        CallArgs::new()
    );
    assert_eq!(
        unstable::named_values_from_parts(MontyGraph::new(), vec![]).unwrap(),
        NamedValues::new()
    );
}

/// Ordinary builders and iteration use values rather than arena ids.
#[test]
fn value_builders_and_borrowed_iteration() {
    let mut args = CallArgs::new();
    let () = args.push_arg(MontyObject::int(42));
    let () = args.push_kwarg("flag", MontyObject::bool(true));
    assert_eq!(args.args().len(), 1);
    assert_eq!(args.kwargs().len(), 1);
    assert_eq!(args.arg(0).unwrap().as_int(), Some(42));
    assert_eq!(args.kwarg("flag").unwrap().as_bool(), Some(true));

    let mut named = NamedValues::new();
    let () = named.push("answer", MontyObject::int(42));
    assert_eq!(named.iter().next().unwrap(), ("answer", args.arg(0).unwrap()));
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
    assert_eq!(list.decoded_size(), size_of::<MontyNode>() + size_of::<NodeId>());
    assert_eq!(
        unstable::graph_parts(&value).0.decoded_size(),
        size_of::<MontyNode>() + 3 + list.decoded_size()
    );
}

// === serialization ===

#[test]
fn arena_round_trips_through_serde() {
    let value = doubling_ladder(2);
    let json = serde_json::to_string(&value).unwrap();
    assert_eq!(serde_json::from_str::<MontyObject>(&json).unwrap(), value);
    let bytes = minicbor_serde::to_vec(&value).unwrap();
    assert_eq!(minicbor_serde::from_slice::<MontyObject>(&bytes).unwrap(), value);
}

/// A list nested `depth` times around `1`, built without recursion.
fn deep_chain(depth: usize) -> MontyObject {
    let mut graph = MontyGraph::new();
    let mut root = graph.push(MontyNode::Int(1));
    for _ in 0..depth {
        root = graph.push(MontyNode::List(vec![root]));
    }
    unstable::object_from_graph(graph, root).unwrap()
}

/// Copying a value out of an arena and rendering its repr walk the arena
/// without recursing, so a deep value from an untrusted worker cannot
/// overflow the host's stack.
#[test]
fn deep_values_copy_and_render_without_recursion() {
    let depth = 200_000;
    let chain = deep_chain(depth);
    let copy = chain.as_ref().to_owned();
    assert_eq!(unstable::graph_parts(&copy).0.len(), depth + 1);
    assert_eq!(copy, chain);
    let repr = chain.py_repr();
    assert_eq!(repr.len(), 2 * depth + 1);
    assert_eq!(&repr[depth - 1..=depth + 1], "[1]");
}

/// Copying a value out of a merged arena takes only its own nodes, wherever
/// in the arena it sits.
#[test]
fn to_owned_copies_only_the_value() {
    let mut call = CallArgs::new();
    call.push_arg(MontyObject::list((0..100).map(MontyObject::int)));
    let shared = MontyObject::list([MontyObject::int(1)]);
    call.push_arg(MontyObject::tuple([shared.clone(), shared]));
    let copy = call.arg(1).unwrap().to_owned();
    assert_eq!(copy.py_repr(), "([1], [1])");
    assert_eq!(unstable::graph_parts(&copy).0.len(), 5);
}
