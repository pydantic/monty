//! [`ToArgs`] / [`PushValue`] — projection of typed args structs into the
//! [`CallArgs`] host callbacks consume. The `#[derive(ToArgs)]` macro in
//! `monty-macros` emits impls of these traits via `crate::args::…` paths,
//! which resolve in this crate.

use num_bigint::BigInt;

use crate::{
    file_mode::FileMode,
    graph::{MontyGraph, MontyNode, NodeId},
    object::CallArgs,
};
/// Projects a typed args struct into the [`CallArgs`] host callbacks expect.
/// Consumes `self` to avoid cloning owned fields.
///
/// Inverse of `monty`'s internal `FromArgs` (`ArgValues` → struct); [`ToArgs`]
/// is struct → host-facing `(args, kwargs)`. Driven by
/// [`crate::os::OsFunctionCall::to_args`] for the monty-python / monty-js bindings.
pub trait ToArgs {
    fn to_args(self) -> CallArgs;
}
/// Consume `self` into a node of `graph`, returning its id.
///
/// Implementers shape themselves into the most natural [`MontyNode`] —
/// `String` → [`MontyNode::String`], `Vec<u8>` → [`MontyNode::Bytes`], etc.
/// A composite value pushes its children first so the arena stays post-order.
pub trait PushValue {
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
