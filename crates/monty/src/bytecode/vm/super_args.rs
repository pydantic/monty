//! The arguments zero-argument `super()` reads from the calling frame.

use super::VM;
use crate::{
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::HeapData,
    value::Value,
};

impl VM<'_> {
    /// `super()`'s implicit `(__class__, first argument)` pair, both owned.
    ///
    /// CPython reads them from the calling frame: the `__class__` cell the
    /// compiler gives any method mentioning `super`, and local 0, which may
    /// itself sit in a cell when a closure captures it.
    pub(crate) fn zero_arg_super(&mut self) -> RunResult<(Value, Value)> {
        let frame = self.current_frame();
        let base = frame.stack_base();
        let Some(func_id) = frame.function_id else {
            return Err(ExcType::runtime_error("super(): no arguments"));
        };
        let func = self.interns.get_function(func_id);
        if !func.signature.has_positional_param() {
            return Err(ExcType::runtime_error("super(): no arguments"));
        }
        // Local 0 holds the first argument, unless a closure captured it into a cell.
        let first_cell = func
            .cell_param_indices
            .iter()
            .position(|param| *param == Some(0))
            .map(|index| func.cell_var_slots[index].index());
        let first = match first_cell {
            Some(slot) => self.cell_contents(&self.stack[base + slot]),
            None => self.stack[base].clone_with_heap(self.heap),
        };
        if matches!(first, Value::Undefined) {
            return Err(ExcType::runtime_error("super(): arg[0] deleted"));
        }
        let class_slot = func
            .free_var_slots
            .iter()
            .find(|slot| {
                func.code
                    .local_name(slot.as_u16())
                    .is_some_and(|name| self.interns.get_str(name) == "__class__")
            })
            .map(|slot| slot.index());
        let Some(class_slot) = class_slot else {
            first.drop_with(self);
            return Err(ExcType::runtime_error("super(): __class__ cell not found"));
        };
        let class = self.cell_contents(&self.stack[base + class_slot]);
        if matches!(class, Value::Undefined) {
            first.drop_with(self);
            return Err(ExcType::runtime_error("super(): empty __class__ cell"));
        }
        Ok((class, first))
    }

    /// The value inside a cell slot, owned.
    fn cell_contents(&self, slot: &Value) -> Value {
        match slot {
            Value::Ref(id) if let HeapData::Cell(cell) = self.heap.get(*id) => cell.0.clone_with_heap(self.heap),
            _ => unreachable!("captured slots hold cells"),
        }
    }
}
