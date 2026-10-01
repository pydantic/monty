//! Representation access for bindings and transport adapters.
//!
//! These APIs expose the current storage of boundary values and carry no API
//! compatibility guarantee: they may change or be removed in any release.
//! Prefer [`MontyObject::as_ref`] and its value accessors when possible.

use num_bigint::BigInt;

use super::{CallArgs, MontyObject, NamedValues, ObjectRef};
pub use crate::graph::{ClassTypeNode, GraphError, MontyGraph, MontyNode, NodeId};
use crate::{file_mode::FileMode, os::TimeCaller};

/// The owned arena, positional roots and keyword roots of a call.
pub type CallArgsParts = (MontyGraph, Vec<NodeId>, Vec<(NodeId, NodeId)>);
/// Borrowed call storage, without copying nodes or roots.
pub type CallArgsPartsRef<'a> = (&'a MontyGraph, &'a [NodeId], &'a [(NodeId, NodeId)]);
/// Mutable call storage for graph-level construction.
pub type CallArgsPartsMut<'a> = (&'a mut MontyGraph, &'a mut Vec<NodeId>, &'a mut Vec<(NodeId, NodeId)>);

/// Borrows the current arena and its root without copying.
/// This exposes storage rather than a stable value interface.
///
/// ```
/// use monty_types::{MontyObject, unstable};
/// let value = MontyObject::int(42);
/// let (graph, root) = unstable::graph_parts(&value);
/// assert_eq!(graph.value(root), value.as_ref());
/// ```
#[must_use]
pub fn graph_parts(value: &MontyObject) -> (&MontyGraph, NodeId) {
    (&value.graph, value.root)
}

/// Takes the current arena and root without copying.
/// Rebuild an edited graph with [`object_from_graph`] to check the root pairing.
#[must_use]
pub fn into_graph_parts(value: MontyObject) -> (MontyGraph, NodeId) {
    (value.graph, value.root)
}

/// Pairs an arena with a root, checking the root is in range.
pub fn object_from_graph(graph: MontyGraph, root: NodeId) -> Result<MontyObject, GraphError> {
    MontyObject::new(graph, root)
}

/// Builds a value from a single node.
///
/// # Panics
/// If the node holds child ids or otherwise fails graph validation.
#[must_use]
pub fn object_from_node(node: MontyNode) -> MontyObject {
    MontyObject::leaf(node)
}

/// Borrows the stored root node, exposing its arena-relative child ids.
#[must_use]
pub fn root_node(value: &MontyObject) -> &MontyNode {
    value.root_node()
}

/// Borrows the stored node of a value view, exposing its arena-relative child ids.
#[must_use]
pub fn node(value: ObjectRef<'_>) -> &MontyNode {
    value.node()
}

/// Borrows another node from the view's arena.
///
/// # Panics
/// If `id` is out of range; ids must come from this arena.
#[must_use]
pub fn child(value: ObjectRef<'_>, id: NodeId) -> ObjectRef<'_> {
    value.child(id)
}

/// Borrows the arena and roots of a call without copying.
#[must_use]
pub fn call_args_parts(args: &CallArgs) -> CallArgsPartsRef<'_> {
    (&args.graph, &args.arg_ids, &args.kwarg_ids)
}

/// Borrows call storage for graph-level construction.
/// Restore valid roots before inspecting or transmitting the arguments.
#[must_use]
pub fn call_args_parts_mut(args: &mut CallArgs) -> CallArgsPartsMut<'_> {
    (&mut args.graph, &mut args.arg_ids, &mut args.kwarg_ids)
}

/// Takes the arena and roots of a call without copying.
#[must_use]
pub fn into_call_args_parts(args: CallArgs) -> CallArgsParts {
    (args.graph, args.arg_ids, args.kwarg_ids)
}

/// Builds a call from graph storage, rejecting out-of-range argument roots.
pub fn call_args_from_parts(
    graph: MontyGraph,
    arg_ids: Vec<NodeId>,
    kwarg_ids: Vec<(NodeId, NodeId)>,
) -> Result<CallArgs, GraphError> {
    let args = CallArgs {
        graph,
        arg_ids,
        kwarg_ids,
    };
    args.check_roots()?;
    Ok(args)
}

/// Borrows the arena and named roots of a feed without copying.
#[must_use]
pub fn named_values_parts(values: &NamedValues) -> (&MontyGraph, &[(String, NodeId)]) {
    (&values.graph, &values.names)
}

/// Takes the arena and named roots of a feed without copying.
#[must_use]
pub fn into_named_values_parts(values: NamedValues) -> (MontyGraph, Vec<(String, NodeId)>) {
    (values.graph, values.names)
}

/// Builds feed inputs from graph storage, rejecting out-of-range value roots.
pub fn named_values_from_parts(graph: MontyGraph, names: Vec<(String, NodeId)>) -> Result<NamedValues, GraphError> {
    let values = NamedValues { graph, names };
    values.check_roots()?;
    Ok(values)
}

/// Appends an argument directly to the call's arena and returns its node id.
pub fn push_arg(args: &mut CallArgs, value: impl PushValue) -> NodeId {
    let id = value.push_into(&mut args.graph);
    args.arg_ids.push(id);
    id
}

/// Appends a keyword argument directly to the call's arena.
pub fn push_kwarg(args: &mut CallArgs, name: &str, value: impl PushValue) {
    let key = args.graph.push(MontyNode::String(name.to_owned()));
    let value = value.push_into(&mut args.graph);
    args.kwarg_ids.push((key, value));
}

/// Appends a named value directly to the feed's arena and returns its node id.
pub fn push_named(values: &mut NamedValues, name: impl Into<String>, value: impl PushValue) -> NodeId {
    let id = value.push_into(&mut values.graph);
    values.names.push((name.into(), id));
    id
}

/// Consumes `self` into a node of `graph`, returning its id.
/// Composite values push their children first so the arena stays post-order.
pub trait PushValue {
    /// Appends a value with its children below it, returning its arena-relative id.
    fn push_into(self, graph: &mut MontyGraph) -> NodeId;
}

impl PushValue for MontyNode {
    fn push_into(self, graph: &mut MontyGraph) -> NodeId {
        graph.push(self)
    }
}

impl PushValue for String {
    fn push_into(self, graph: &mut MontyGraph) -> NodeId {
        graph.push(MontyNode::String(self))
    }
}

impl PushValue for Vec<u8> {
    fn push_into(self, graph: &mut MontyGraph) -> NodeId {
        graph.push(MontyNode::Bytes(self))
    }
}

impl PushValue for i64 {
    fn push_into(self, graph: &mut MontyGraph) -> NodeId {
        graph.push(MontyNode::Int(self))
    }
}

/// Counts above `i64::MAX` cross as `BigInt`, so a host handler receives the
/// exact value and its own cap decides what to do with it.
impl PushValue for u64 {
    fn push_into(self, graph: &mut MontyGraph) -> NodeId {
        graph.push(i64::try_from(self).map_or_else(|_| MontyNode::BigInt(BigInt::from(self)), MontyNode::Int))
    }
}

impl PushValue for bool {
    fn push_into(self, graph: &mut MontyGraph) -> NodeId {
        graph.push(MontyNode::Bool(self))
    }
}

impl PushValue for FileMode {
    fn push_into(self, graph: &mut MontyGraph) -> NodeId {
        graph.push(MontyNode::String(self.as_str().to_owned()))
    }
}

impl PushValue for TimeCaller {
    fn push_into(self, graph: &mut MontyGraph) -> NodeId {
        graph.push(MontyNode::String(self.as_str().to_owned()))
    }
}
