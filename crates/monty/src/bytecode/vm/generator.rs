//! Creation and synchronous advancement of generator-expression frames.

use std::mem;

use super::{CallFrame, FrameExit, VM, recursion::RunReentryGuard};
use crate::{
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{DropGuard, DropWithContext, HeapData, HeapId, HeapReadOutput},
    intern::FunctionId,
    types::{Generator, GeneratorState},
    value::Value,
};

impl VM<'_> {
    /// Builds a generator from the iterator and captured cells on the operand stack.
    pub(super) fn make_generator(&mut self, function_id: FunctionId, cell_count: usize) -> RunResult<()> {
        let cells = self.pop_n(cell_count);
        let iterator = self.pop();
        let mut inputs_guard = DropGuard::new((cells, iterator), self);
        let cell_ids = {
            let ((cells, _), this) = inputs_guard.as_parts();
            let mut cell_ids = Vec::with_capacity(cell_count);
            for cell in cells {
                let Value::Ref(id) = cell else {
                    return Err(RunError::internal("MakeGenerator: expected cell reference on stack"));
                };
                if !matches!(this.heap.get(*id), HeapData::Cell(_)) {
                    return Err(RunError::internal("MakeGenerator: expected captured Cell"));
                }
                cell_ids.push(*id);
            }
            cell_ids
        };

        let ((cells, iterator), this) = inputs_guard.into_parts();
        let function = this.interns.get_function(function_id);
        let mut namespace_guard = DropGuard::new(vec![iterator], this);
        let (namespace, this) = namespace_guard.as_parts_mut();
        this.install_closure_cells(function, &cell_ids, namespace);
        cells.drop_with(this);

        let (namespace, this) = namespace_guard.into_parts();
        let id = this
            .heap
            .allocate(HeapData::Generator(Generator::new(function_id, namespace)));
        this.push(Value::Ref(id));
        Ok(())
    }

    /// Advances a generator until it yields, returns, or raises.
    pub(crate) fn resume_generator(&mut self, generator_id: HeapId) -> RunResult<Option<Value>> {
        let (function_id, ip, stack) = {
            let HeapReadOutput::Generator(mut generator) = self.heap.read(generator_id) else {
                return Err(RunError::internal("generator id does not reference a Generator"));
            };
            let generator = generator.get_mut(self.heap);
            let state = mem::replace(&mut generator.state, GeneratorState::Running);
            match state {
                GeneratorState::New { stack } => (generator.function_id, 0, stack),
                GeneratorState::Suspended { ip, stack } => (generator.function_id, ip, stack),
                GeneratorState::Running => {
                    generator.state = GeneratorState::Running;
                    return Err(ExcType::generator_already_executing());
                }
                GeneratorState::Closed => {
                    generator.state = GeneratorState::Closed;
                    return Ok(None);
                }
            }
        };

        if let Err(error) = self.enter_run_reentry() {
            stack.drop_with(self);
            self.close_generator(generator_id);
            return Err(error.into());
        }
        let mut guard = RunReentryGuard::new(self);
        let this = &mut *guard;
        let call_offset = this.current_offset();
        let stack_base = this.stack.len();
        this.stack.extend(stack);

        let function = this.interns.get_function(function_id);
        let locals_count = u16::try_from(function.namespace_size).expect("generator namespace exceeds u16");
        let mut frame = CallFrame::new_function(
            &function.code,
            stack_base,
            locals_count,
            this.exception_stack.len(),
            function_id,
            call_offset,
        );
        frame.ip = ip;
        frame.should_return = true;
        frame.generator_id = Some(generator_id);
        if let Err(error) = this.push_frame(frame) {
            this.close_generator(generator_id);
            return Err(error);
        }

        let result = this.run();
        match result {
            Ok(FrameExit::Return(value)) => {
                let suspended = {
                    let HeapReadOutput::Generator(generator) = this.heap.read(generator_id) else {
                        unreachable!("generator changed heap type while running")
                    };
                    matches!(generator.get(this.heap).state, GeneratorState::Suspended { .. })
                };
                if suspended {
                    Ok(Some(value))
                } else {
                    value.drop_with(this);
                    this.close_generator(generator_id);
                    Ok(None)
                }
            }
            Ok(exit) => {
                let error = this.unsupported_frame_exit("generator expression", exit);
                let mut error = this.unwind_generator_exit(error);
                this.close_generator(generator_id);
                this.add_generator_caller_frame(&mut error, call_offset);
                Err(error)
            }
            Err(mut error) => {
                this.close_generator(generator_id);
                if error.is_stop_iteration()
                    && let RunError::Exc(exception) = &mut error
                {
                    exception.exc = ExcType::generator_raised_stop_iteration();
                }
                this.add_generator_caller_frame(&mut error, call_offset);
                Err(error)
            }
        }
    }

    /// Moves the current generator frame's stack region back into its heap state.
    pub(super) fn suspend_generator(&mut self, ip: usize) -> RunResult<()> {
        let Some(generator_id) = self.current_frame.generator_id else {
            return Err(RunError::internal("YieldValue outside a generator frame"));
        };
        debug_assert_eq!(self.exception_stack.len(), self.current_frame.exception_stack_base);
        let stack = self.stack.split_off(self.current_frame.stack_base);
        let HeapReadOutput::Generator(mut generator) = self.heap.read(generator_id) else {
            return Err(RunError::internal("generator frame owner changed heap type"));
        };
        let generator = generator.get_mut(self.heap);
        debug_assert!(matches!(generator.state, GeneratorState::Running));
        generator.state = GeneratorState::Suspended { ip, stack };

        let caller = self.suspended_frames.pop().expect("generator frame has no caller");
        self.current_frame = caller;
        self.instruction_ip = self.current_frame.ip;
        if !self.current_frame.is_parked {
            self.decr_recursion();
        }
        Ok(())
    }

    /// Closes a generator after its VM-owned frame state has been cleaned up.
    fn close_generator(&mut self, generator_id: HeapId) {
        let HeapReadOutput::Generator(mut generator) = self.heap.read(generator_id) else {
            return;
        };
        let generator = generator.get_mut(self.heap);
        debug_assert!(matches!(
            generator.state,
            GeneratorState::Running | GeneratorState::Closed
        ));
        if matches!(generator.state, GeneratorState::Running) {
            generator.state = GeneratorState::Closed;
        }
    }
}
