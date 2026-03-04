//! Runtime-observer plumbing for the VM.
//!
//! This module isolates observer-specific VM extensions so upstream VM changes
//! remain easier to sync.

use super::{CallFrame, VM, VMContext, VMSnapshot};
use crate::{
    bytecode::code::Code,
    observer::{
        ControlConditionEvent, OpInputIds, OpResultEvent, RuntimeObserverEvent, RuntimeObserverHandle,
        ValueCreatedEvent,
    },
    resource::ResourceTracker,
    runtime_id::RuntimeValueId,
    value::Value,
};

impl<'a, 'p, T: ResourceTracker> VM<'a, 'p, T> {
    /// Creates a new VM without an observer.
    pub fn new(context: VMContext<'a, 'p, T>) -> Self {
        Self::new_with_observer(context, RuntimeObserverHandle::disabled())
    }

    /// Creates a new VM with an optional runtime observer.
    pub fn new_with_observer(context: VMContext<'a, 'p, T>, observer: RuntimeObserverHandle) -> Self {
        let VMContext {
            heap,
            namespaces,
            interns,
            print_writer,
        } = context;
        Self {
            stack: Vec::with_capacity(64),
            frames: Vec::with_capacity(16),
            heap,
            namespaces,
            interns,
            print_writer,
            exception_stack: Vec::new(),
            instruction_ip: 0,
            next_call_id: 0,
            scheduler: None, // Lazy - no allocation for sync code
            module_code: None,
            observer,
        }
    }

    /// Reconstructs a VM from a snapshot with an optional runtime observer.
    pub fn restore_with_observer(
        snapshot: VMSnapshot,
        module_code: &'a Code,
        context: VMContext<'a, 'p, T>,
        observer: RuntimeObserverHandle,
    ) -> Self {
        // Reconstruct call frames from serialized form
        let frames = snapshot
            .frames
            .into_iter()
            .map(|sf| {
                let code = match sf.function_id {
                    Some(func_id) => &context.interns.get_function(func_id).code,
                    None => module_code,
                };
                CallFrame {
                    code,
                    ip: sf.ip,
                    stack_base: sf.stack_base,
                    namespace_idx: sf.namespace_idx,
                    function_id: sf.function_id,
                    cells: sf.cells,
                    call_position: sf.call_position,
                    should_return: false,
                }
            })
            .collect();
        let VMContext {
            heap,
            namespaces,
            interns,
            print_writer,
        } = context;

        Self {
            stack: snapshot.stack,
            frames,
            heap,
            namespaces,
            interns,
            print_writer,
            exception_stack: snapshot.exception_stack,
            instruction_ip: snapshot.instruction_ip,
            next_call_id: snapshot.next_call_id,
            scheduler: snapshot.scheduler,
            module_code: Some(module_code),
            observer,
        }
    }

    /// Emits a value-creation observer event for a stack value.
    #[inline]
    pub(super) fn emit_value_created(&self, value: &Value) {
        if !self.observer.is_enabled() {
            return;
        }
        self.observer
            .emit(RuntimeObserverEvent::ValueCreated(ValueCreatedEvent {
                value_id: RuntimeValueId::new(value.id()),
            }));
    }

    /// Emits an operation-result observer event.
    #[inline]
    pub(super) fn emit_op_result(&self, output: &Value, inputs: OpInputIds) {
        if !self.observer.is_enabled() {
            return;
        }
        self.observer.emit(RuntimeObserverEvent::OpResult(OpResultEvent {
            output_id: RuntimeValueId::new(output.id()),
            inputs,
        }));
    }

    /// Emits a unary operation-result event.
    #[inline]
    pub(super) fn emit_unary_op_result(&self, input_id: Option<RuntimeValueId>, output: &Value) {
        if !self.observer.is_enabled() {
            return;
        }
        let Some(input_id) = input_id else {
            return;
        };
        self.emit_op_result(output, OpInputIds::One(input_id));
    }

    /// Emits a binary operation-result event.
    #[inline]
    pub(super) fn emit_binary_op_result(&self, lhs: &Value, rhs: &Value, output: &Value) {
        if !self.observer.is_enabled() {
            return;
        }
        self.emit_op_result(
            output,
            OpInputIds::Two(RuntimeValueId::new(lhs.id()), RuntimeValueId::new(rhs.id())),
        );
    }

    /// Emits a control-condition observer event.
    #[inline]
    pub(super) fn emit_control_condition(&self, condition: &Value, branch_taken: bool) {
        if !self.observer.is_enabled() {
            return;
        }
        self.observer
            .emit(RuntimeObserverEvent::ControlCondition(ControlConditionEvent {
                condition_id: RuntimeValueId::new(condition.id()),
                branch_taken,
            }));
    }
}
