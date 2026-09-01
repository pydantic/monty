//! Runtime state for lazy generator expressions.

use std::fmt::Write;

use serde::{Deserialize, Serialize};

use crate::{
    bytecode::VM,
    exception_private::RunResult,
    hash::{HashValue, identity_hash},
    heap::{HeapId, HeapItem, HeapObjectRead},
    intern::FunctionId,
    types::{LazyHeapSet, PyTrait, Type},
    value::Value,
};

/// Saved execution state for a generator expression.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum GeneratorState {
    /// Created but not yet advanced; `stack` contains its complete frame region.
    New { stack: Vec<Value> },
    /// Currently executing; its frame region has moved onto the VM stack.
    Running,
    /// Paused immediately after yielding a value.
    Suspended { ip: usize, stack: Vec<Value> },
    /// Permanently exhausted after return or an escaping error.
    Closed,
}

/// A single-pass iterator backed by a resumable synthetic `<genexpr>` frame.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Generator {
    /// Function table entry containing the synthetic bytecode and closure layout.
    pub(crate) function_id: FunctionId,
    /// Current ownership location of the generator frame's values.
    pub(crate) state: GeneratorState,
}

impl Generator {
    /// Creates a generator which owns its initialized frame stack.
    pub(crate) fn new(function_id: FunctionId, stack: Vec<Value>) -> Self {
        Self {
            function_id,
            state: GeneratorState::New { stack },
        }
    }

    /// Invokes `on_child` once for every owned heap reference in saved state.
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        let stack = match &self.state {
            GeneratorState::New { stack } | GeneratorState::Suspended { stack, .. } => stack,
            GeneratorState::Running | GeneratorState::Closed => return,
        };
        for value in stack {
            if let Value::Ref(id) = value {
                on_child(*id);
            }
        }
    }
}

impl HeapItem for Generator {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        let values = match &mut self.state {
            GeneratorState::New { stack } | GeneratorState::Suspended { stack, .. } => stack,
            GeneratorState::Running | GeneratorState::Closed => return,
        };
        for value in values {
            value.py_dec_ref_ids(stack);
        }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, Generator> {
    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::Generator
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_hash(&self, _: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_is_iterable(&self, _: &VM<'h>) -> bool {
        true
    }

    fn py_is_iterator(&self, _: &VM<'h>) -> bool {
        true
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        Ok(self.clone_value(vm.heap))
    }

    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        vm.resume_generator(self.id())
    }

    fn py_repr_fmt(&self, f: &mut impl Write, _: &mut VM<'h>, _: &mut LazyHeapSet) -> RunResult<()> {
        f.write_str("<generator object <genexpr>>")?;
        Ok(())
    }
}
