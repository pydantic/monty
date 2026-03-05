//! Runtime-observer plumbing for the VM.
//!
//! This module isolates observer-specific VM extensions so upstream VM changes
//! remain easier to sync.

use super::{VM, VMSnapshot};
use crate::{
    bytecode::code::Code,
    heap::Heap,
    intern::Interns,
    io::PrintWriter,
    namespace::Namespaces,
    observer::{
        ControlConditionEvent, OpInputIds, OpResultEvent, RuntimeObserverEvent, RuntimeObserverHandle,
        ValueCreatedEvent,
    },
    resource::ResourceTracker,
    runtime_id::RuntimeValueId,
    value::Value,
};

impl<'a, 'p, T: ResourceTracker> VM<'a, 'p, T> {
    /// Creates a new VM with an optional runtime observer.
    pub fn new_with_observer(
        heap: &'a mut Heap<T>,
        namespaces: &'a mut Namespaces,
        interns: &'a Interns,
        print_writer: &'a mut PrintWriter<'p>,
        observer: RuntimeObserverHandle,
    ) -> Self {
        let mut vm = Self::new(heap, namespaces, interns, print_writer);
        vm.observer = observer;
        vm
    }

    /// Reconstructs a VM from a snapshot with an optional runtime observer.
    pub fn restore_with_observer(
        snapshot: VMSnapshot,
        module_code: &'a Code,
        heap: &'a mut Heap<T>,
        namespaces: &'a mut Namespaces,
        interns: &'a Interns,
        print_writer: &'a mut PrintWriter<'p>,
        observer: RuntimeObserverHandle,
    ) -> Self {
        let mut vm = Self::restore(snapshot, module_code, heap, namespaces, interns, print_writer);
        vm.observer = observer;
        vm
    }

    /// Pushes a value while emitting a `ValueCreated` event when observation is enabled.
    #[inline]
    pub(crate) fn push_created(&mut self, value: Value) {
        self.emit_value_created(&value);
        self.push(value);
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
