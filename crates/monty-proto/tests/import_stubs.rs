use monty_proto::{import_stubs_from_answer, pb, validate_import_stubs_answer};
use monty_types::{
    ModuleStub, MontyObject,
    unstable::{MontyGraph, MontyNode, object_from_graph},
};

fn shared_answer(entries: usize, source_len: usize) -> MontyObject {
    let mut graph = MontyGraph::new();
    let source = graph.push(MontyNode::String("x".repeat(source_len)));
    let pairs = (0..entries)
        .map(|i| {
            let key = graph.push(MontyNode::String(format!("tools.m{i:03}")));
            (key, source)
        })
        .collect();
    let root = graph.push(MontyNode::Dict(pairs));
    object_from_graph(graph, root).unwrap()
}

#[test]
fn valid_shared_sources_produce_all_stubs() {
    let answer = shared_answer(3, 16);
    let asked = vec!["tools".to_owned()];
    validate_import_stubs_answer(&answer, &asked).unwrap();
    let stubs = import_stubs_from_answer(&answer, &asked).unwrap();
    assert_eq!(stubs.len(), 3);
    for (i, stub) in stubs.iter().enumerate() {
        assert_eq!(stub.module(), format!("tools.m{i:03}"));
        assert_eq!(stub.source(), "x".repeat(16));
    }
}

#[test]
fn expanded_storage_accepts_exactly_64_mib() {
    let asked = vec!["tools".to_owned()];
    // Each tools.mNNN entry charges its name, two package paths and vector storage.
    // Use target-specific structure sizes so the boundary also holds on 32-bit targets.
    let entry_bytes = "tools.m000".len()
        + "tools/__init__.pyi".len()
        + "tools/m000/__init__.pyi".len()
        + 2 * (size_of::<(&str, &str)>() + size_of::<ModuleStub>())
        + 4 * size_of::<(String, &str)>();
    let source_len = (1 << 20) - entry_bytes;
    let answer = shared_answer(64, source_len);
    let peak = allocation_counter::measure(|| validate_import_stubs_answer(&answer, &asked).unwrap());
    assert!(peak.bytes_max < 1 << 20, "validation copied sources: {peak:?}");
    assert_eq!(import_stubs_from_answer(&answer, &asked).unwrap().len(), 64);

    // One extra source byte per entry puts the estimate 64 bytes over the limit.
    let oversized = shared_answer(64, source_len + 1);
    assert_rejected_with_peak_below(&oversized, 1 << 20);
}

#[test]
fn deep_module_below_path_budget_produces_stub() {
    let module = format!("tools{}", ".a".repeat(1_000));
    let answer = MontyObject::dict([(MontyObject::string(&module), MontyObject::string(""))]);
    let asked = vec!["tools".to_owned()];
    validate_import_stubs_answer(&answer, &asked).unwrap();
    let stubs = import_stubs_from_answer(&answer, &asked).unwrap();
    assert_eq!(stubs.len(), 1);
    assert_eq!(stubs[0].module(), module);
    assert_eq!(stubs[0].source(), "");
}

#[test]
fn ancestor_paths_exceed_budget_before_path_allocation() {
    let module = format!("tools{}", ".a".repeat(10_000));
    let answer = MontyObject::dict([(MontyObject::string(module), MontyObject::string(""))]);
    assert_rejected_with_peak_below(&answer, 1 << 20);
}

#[test]
fn tiny_distinct_entries_exceed_budget_before_owned_stub_allocation() {
    let answer = shared_answer(600_000, 0);
    assert_rejected_with_peak_below(&answer, 16 << 20);
}

#[test]
fn duplicate_raw_dict_entries_count_toward_storage_budget() {
    let mut graph = MontyGraph::new();
    let key = graph.push(MontyNode::String("tools".to_owned()));
    let source = graph.push(MontyNode::String(String::new()));
    let root = graph.push(MontyNode::Dict(vec![(key, source); 2 << 20]));
    let answer = object_from_graph(graph, root).unwrap();
    assert_rejected_with_peak_below(&answer, 16 << 20);
}

#[test]
fn none_entries_do_not_consume_stub_storage_budget() {
    let module = format!("tools{}", ".a".repeat(10_000));
    let answer = MontyObject::dict([(MontyObject::string(module), MontyObject::none())]);
    let asked = vec!["tools".to_owned()];
    validate_import_stubs_answer(&answer, &asked).unwrap();
    assert_eq!(import_stubs_from_answer(&answer, &asked).unwrap(), vec![]);
}

#[test]
fn many_none_entries_validate_without_pair_allocation() {
    let mut graph = MontyGraph::new();
    let key = graph.push(MontyNode::String("tools".to_owned()));
    let value = graph.push(MontyNode::None);
    let root = graph.push(MontyNode::Dict(vec![(key, value); 2 << 20]));
    let answer = object_from_graph(graph, root).unwrap();
    let asked = vec!["tools".to_owned()];
    let peak = allocation_counter::measure(|| {
        validate_import_stubs_answer(&answer, &asked).unwrap();
        assert_eq!(import_stubs_from_answer(&answer, &asked).unwrap(), vec![]);
    });
    assert!(peak.bytes_max < 1 << 20, "{peak:?}");
}

/// Both consumers must reject before materializing the reply's expanded storage.
fn assert_rejected_with_peak_below(answer: &MontyObject, max_bytes: u64) {
    let asked = vec!["tools".to_owned()];
    let validation = allocation_counter::measure(|| {
        assert_eq!(
            validate_import_stubs_answer(answer, &asked).unwrap_err(),
            "import_stubs estimated expanded storage exceeds 64 MiB"
        );
    });
    let conversion = allocation_counter::measure(|| {
        assert_eq!(
            import_stubs_from_answer(answer, &asked).unwrap_err(),
            "import_stubs estimated expanded storage exceeds 64 MiB"
        );
    });
    assert!(validation.bytes_max < max_bytes, "{validation:?}");
    assert!(conversion.bytes_max < max_bytes, "{conversion:?}");
}

#[test]
fn oversized_shared_reply_is_rejected_without_expanding_it() {
    let answer = shared_answer(100, 1 << 20);
    let asked = vec!["tools".to_owned()];
    // Construct the shared graph before measuring either consumer's allocations.
    let validation = allocation_counter::measure(|| {
        assert!(
            validate_import_stubs_answer(&answer, &asked)
                .unwrap_err()
                .contains("64 MiB")
        );
    });
    let conversion = allocation_counter::measure(|| {
        assert!(
            import_stubs_from_answer(&answer, &asked)
                .unwrap_err()
                .contains("64 MiB")
        );
    });
    assert!(validation.bytes_max < 1 << 20, "{validation:?}");
    assert!(conversion.bytes_max < 1 << 20, "{conversion:?}");

    let (result, values) = monty_proto::ext_result_to_proto(monty_types::ExtFunctionResult::Return(answer));
    let request = pb::ClientRequest {
        kind: Some(pb::client_request::Kind::ResumeCall(pb::ResumeCall {
            call_id: 1,
            result: Some(result),
            values,
        })),
        ..Default::default()
    };
    let encoded = monty_proto::encode_to_capped_vec(&request).unwrap();
    assert!(encoded.len() < 2 << 20, "shared source should fit on the wire");
}

#[test]
fn both_consumers_validate_shapes_paths_and_requested_modules() {
    let asked = vec!["tools".to_owned(), "builtins".to_owned()];
    for answer in [
        MontyObject::none(),
        MontyObject::dict([(MontyObject::none(), MontyObject::none())]),
        MontyObject::dict([(MontyObject::string("other"), MontyObject::none())]),
        MontyObject::dict([(MontyObject::string("tools"), MontyObject::bool(true))]),
        MontyObject::dict([(MontyObject::string("tools.1bad"), MontyObject::string(""))]),
        MontyObject::dict([(MontyObject::string("builtins"), MontyObject::string(""))]),
    ] {
        let validation = validate_import_stubs_answer(&answer, &asked).unwrap_err();
        assert_eq!(validation, import_stubs_from_answer(&answer, &asked).unwrap_err());
    }
    // None does not create a file, so only membership in the request is checked.
    let absent = MontyObject::dict([(MontyObject::string("tools.1bad"), MontyObject::none())]);
    validate_import_stubs_answer(&absent, &asked).unwrap();
    assert_eq!(import_stubs_from_answer(&absent, &asked).unwrap(), vec![]);
}
