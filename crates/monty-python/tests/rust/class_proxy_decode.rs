//! Host memory for class instance proxies decoded from one worker message.

use allocation_counter::measure;
use monty_proto::python::{DecodedArena, InstanceStore};
use monty_types::{
    MontyUuid,
    unstable::{ClassTypeNode, MontyGraph, MontyNode},
};
use pyo3::prelude::*;

/// Every proxy of one class shares the class name, so a long name is not copied once per instance.
///
/// The decode budget counts the class name once, because the wire carries it once. A copy per
/// instance would let 5,000 instances of a class with a 1 MB name allocate about 5 GB in the host.
#[test]
fn class_instance_proxies_share_one_copy_of_the_class_name() {
    const NAME_BYTES: usize = 100_000;
    const INSTANCES: u128 = 1_000;
    let mut graph = MontyGraph::new();
    let class = graph.push(MontyNode::ClassType(Box::new(ClassTypeNode {
        name: "A".repeat(NAME_BYTES),
        id: MontyUuid::from_u128(1),
        host_defined: false,
        is_dataclass: false,
        attrs: vec![],
    })));
    let instances = (0..INSTANCES)
        .map(|i| {
            graph.push(MontyNode::ClassInstance {
                class_type: class,
                instance_id: MontyUuid::from_u128(i + 2),
                attrs: vec![],
            })
        })
        .collect();
    graph.push(MontyNode::List(instances));

    Python::initialize();
    Python::attach(|py| {
        let store = InstanceStore::new(py);
        let allocated = measure(|| {
            DecodedArena::new(py, &graph, &store).unwrap();
        });
        // One copy per instance allocates INSTANCES * NAME_BYTES (100 MB).
        assert!(
            allocated.bytes_total < 10 * NAME_BYTES as u64,
            "decoding allocated {} bytes",
            allocated.bytes_total
        );
    });
}
