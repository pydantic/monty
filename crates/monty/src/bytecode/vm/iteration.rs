//! Continuation for `ForIter` when advancing cannot finish in the opcode itself.

use super::{CallFrame, CallResult, VM};
use crate::{
    args::{ArgValues, Signature},
    builtins::Builtins,
    bytecode::{
        Code, CodeBuilder, Opcode,
        code::HandlerKind,
        compiler::{CompileError, check_function_count_u16},
    },
    exception_private::{RunError, RunResult},
    expressions::Identifier,
    frozen::{FrozenFunction, FrozenFunctions},
    function::Function,
    heap::DropGuard,
    intern::{CompileInterns, FunctionId},
    modules::ModuleFunctions,
    parse::CodeRange,
    types::Type,
    value::Value,
};

impl VM<'_> {
    /// Insert a catcher *below* the advancement so its own exception handlers see errors first.
    /// The zero-local frame shares the caller's operand boundary, including across snapshots.
    pub(super) fn catch_deferred_iteration(&mut self, call: CallResult, stack_base: usize) -> RunResult<CallResult> {
        let mut guard = DropGuard::new(call, self);
        let (call, vm) = guard.as_parts();
        let pushed = matches!(call, CallResult::FramePushed);
        let id = match FrozenFunction::ForIter.resolve(vm.interns) {
            Ok(id) => id,
            Err(error) => {
                if pushed {
                    let call_site = vm.current_frame.call_offset.expect("advancement has a caller");
                    vm.pop_frame();
                    vm.instruction_ip = call_site as usize;
                    // A constructor may have placed its pending instance below the callee's locals.
                    for value in vm.stack.drain(stack_base..) {
                        value.drop_with(vm.heap);
                    }
                }
                return Err(error);
            }
        };
        let call_site = if pushed {
            vm.current_frame.call_offset.replace(0)
        } else {
            vm.current_offset()
        };
        let catcher = CallFrame::new_function(
            &vm.interns.get_function(id).code,
            stack_base,
            0,
            vm.exception_stack.len(),
            id,
            call_site,
            None,
        );
        if pushed {
            vm.suspended_frames.push(catcher);
        } else {
            vm.push_admitted_frame(catcher);
            vm.instruction_ip = 0;
        }
        Ok(guard.into_inner())
    }

    /// Non-returning tail of the catcher's `except StopIteration` suite. Its caller is a loop,
    /// which expects neither an iterator nor a result at the exit target.
    pub(crate) fn end_for_iter(&mut self, args: ArgValues) -> RunResult<CallResult> {
        args.check_zero_args("end_for_iter", self.heap)?;
        if self.current_frame.function_id.is_none()
            || self.current_frame.function_id != self.interns.frozen_function(FrozenFunction::ForIter)
        {
            return Err(RunError::internal("end_for_iter outside the iteration catcher"));
        }
        let site = self.current_frame.call_offset.expect("iteration catcher has a caller") as usize;
        self.pop_frame();
        let frame = self.current_frame_mut();
        debug_assert_eq!(frame.bytecode[site], Opcode::ForIter as u8);
        frame.ip = site + 1;
        let offset = frame.fetch_i16();
        frame.ip = frame
            .ip
            .checked_add_signed(offset.into())
            .expect("invalid ForIter target");
        self.pop().drop_with(self);
        Ok(CallResult::CallerRestored)
    }
}

/// Creates the shared frozen function. Dependencies are constants rather than locals, so inserting
/// it underneath an installed callee never moves the callee's locals or any pending constructor result.
pub(crate) fn compile_iteration_catcher(interns: &mut CompileInterns<'_>) -> RunResult<FunctionId> {
    let index = check_function_count_u16(interns.functions_len(), CodeRange::default())
        .map_err(|err| err.into_run_error(""))?;
    let name = Identifier::new(interns.intern("__monty_for_iter"), CodeRange::default());
    interns.push_function(Function::new(
        name,
        Signature::default(),
        0,
        vec![],
        vec![],
        vec![],
        vec![],
        0,
        false,
        catcher_code().expect("fixed iteration catcher bytecode fits"),
    ));
    Ok(FunctionId::from_index(index))
}

/// Offset zero is both the protected call site and the successful return continuation. The item
/// is already on the stack on success. The failure path is an ordinary `except StopIteration`.
fn catcher_code() -> Result<Code, CompileError> {
    let mut code = CodeBuilder::new();
    code.new_code_region(1);
    let site = code.current_offset();
    code.emit(Opcode::ReturnValue)?;
    let handler = code.current_offset();
    code.add_exception_entry(site, handler, handler, 0, 0, HandlerKind::Consuming)?;
    code.new_code_region(1);
    let stop = code.add_const(Value::Builtin(Builtins::Type(Type::StopIteration)))?;
    code.emit_u16(Opcode::LoadConst, stop)?;
    code.emit(Opcode::CheckExcMatch)?;
    let matched = code.emit_jump(Opcode::JumpIfTrue)?;
    code.emit(Opcode::Reraise)?;
    code.patch_jump(matched)?;
    code.emit(Opcode::Pop)?;
    let finish = code.add_const(Value::ModuleFunction(ModuleFunctions::Frozen(
        FrozenFunctions::EndForIter,
    )))?;
    code.emit_u16(Opcode::LoadConst, finish)?;
    code.emit_u8(Opcode::CallFunction, 0)?;
    Ok(code.build())
}
