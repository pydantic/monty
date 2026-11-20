//! Function call helpers for the VM.
//!
//! This module contains the implementation of call-related opcodes and helper
//! functions for executing function calls. The main entry points are the `exec_*`
//! methods which are called from the VM's main dispatch loop.

use std::{array, mem};

use monty_types::{MontyUuid, OsFunctionCall};

use super::{
    CallFrame, FrameGlobals, FrameNamespace, MissingNames, VM, attr::PendingLookupEffect, function_namespace,
    recursion::RunReentryGuard,
};
use crate::{
    args::{ArgValues, KwargsValues, Signature},
    asyncio::Coroutine,
    builtins::{BuiltinsFunctions, BuiltinsFunctionsExt, object::takes_no_arguments},
    bytecode::{FrameExit, Opcode},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    frozen::FrozenFunction,
    function::{ExactPositionalCall, Function},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId},
    heap_data::CellValue,
    intern::{FunctionId, StaticStrings, StringId},
    modules::dataclasses,
    os_dispatch::{PendingEffect, release_pending_effect},
    py_value::PyValue,
    resource_checks::check_estimated_size,
    types::{
        Dict, Instance, PyTrait, Tuple, TupleVec, Type, TypeRef, allocate_tuple,
        instance::{class_defines, class_lookup},
    },
    value::{EitherStr, VALUE_SIZE, Value},
};

/// Result of executing a call or attribute method.
///
/// Used by the `exec_*` methods and `py_call_attr` implementations to communicate
/// what action the VM's main loop should take after the call completes.
///
/// For attribute methods that complete synchronously, use `CallResult::Value`.
/// For operations requiring host involvement (OS calls, external functions, etc.),
/// use the appropriate variant to signal the VM to yield.
pub(crate) enum CallResult {
    /// Call completed synchronously with a return value.
    Value(Value),
    /// A defined function became the VM's current frame.
    /// Propagate or require synchronous completion; dropping this result does not pop the frame.
    FramePushed,
    /// External function call requested - VM should pause and return to caller.
    External {
        /// The external function's name (interned or heap-owned).
        name: EitherStr,
        args: ArgValues,
        /// The callee is an async host function, so a supporting host may
        /// start the call without waiting (see [`crate::types::ExtFunction::is_async`]).
        is_async: bool,
    },
    /// OS operation call requested - VM should yield `FrameExit::OsCall` to host.
    ///
    /// The host executes the OS operation and resumes the VM with the result.
    /// The [`OsFunctionCall`] is a tagged enum whose variants carry their own
    /// typed args, so no separate `ArgValues` is needed at this layer.
    OsCall(OsFunctionCall),
    /// Host-routed call requested - VM should yield `FrameExit::MethodCall`
    /// to host: a method call on a host-backed instance, or on a host class
    /// type — a classmethod, or construction, which is spelled `__call__`.
    ///
    /// The receiver is NOT included in `args`: the host routes the call by
    /// `object_id`, the receiver's uuid (stored on the [`HostClass`] /
    /// [`HostClassType`]). Unlike `External`, `name` is an `EitherStr`
    /// because method names are only known at runtime.
    ///
    /// [`HostClass`]: crate::types::HostClass
    /// [`HostClassType`]: crate::types::HostClassType
    MethodCall {
        name: EitherStr,
        args: ArgValues,
        object_id: MontyUuid,
    },
    /// Lazy attribute lookup on a host-backed object - VM should yield
    /// `FrameExit::AttrLookup` to host.
    ///
    /// Produced by `obj.attr` (or `Type.attr` when `type_object` is true)
    /// and by `getattr()` / `hasattr()` when `attr` is public and missing
    /// from the eager attrs. `class_name` is captured at suspension so the
    /// resume-undefined AttributeError names the real class without touching
    /// the heap.
    AttrLookup {
        name: EitherStr,
        class_name: String,
        object_id: MontyUuid,
        /// True for a lookup on a class type object — selects CPython's
        /// `type object '...' has no attribute ...` message on failure.
        type_object: bool,
        /// How `getattr()` / `hasattr()` consume the answer; `None` for
        /// `obj.attr`. The only heap reference this variant can carry.
        effect: Option<PendingLookupEffect>,
    },
    /// The call returned a value that should be implicitly awaited.
    ///
    /// Used by `asyncio.run()` to execute a coroutine without an explicit `await`.
    /// The VM will push the value onto the stack and execute `exec_get_awaitable`.
    AwaitValue(Value),
    /// OS call whose result must be post-processed on resume via a
    /// [`PendingEffect`] instead of being pushed onto the operand stack raw.
    ///
    /// Used by buffered file reads (`BufferStore`), buffered writes
    /// (`WritePosition`), and `os.listdir` (`ListdirNames`). The effect stays
    /// *in* the value, handed on to `FrameExit::OsCall`, so a call rejected on
    /// the way out cannot corrupt the next OS call's resume.
    OsCallWithEffect {
        call: OsFunctionCall,
        effect: PendingEffect,
    },
}

impl CallResult {
    /// Completes a call at a Rust boundary that cannot yield to the host.
    /// Python frames still run; unsupported suspensions are raised inside them and cleaned up.
    /// Consume immediately after dispatch, before running any of the installed frame's bytecode.
    pub(crate) fn require_synchronous(self, vm: &mut VM<'_>) -> RunResult<Value> {
        if matches!(self, Self::FramePushed) {
            if let Err(error) = vm.enter_run_reentry() {
                // Dispatch already installed this frame, but it has not executed an instruction.
                let instruction_ip = vm.instruction_ip;
                vm.pop_frame();
                vm.instruction_ip = instruction_ip;
                return Err(error.into());
            }
            let mut guard = RunReentryGuard::new(vm);
            self.finish_synchronously(&mut guard)
        } else {
            self.finish_synchronously(vm)
        }
    }

    /// Runs under the caller's native re-entry reservation when a frame needs execution.
    fn finish_synchronously(self, vm: &mut VM<'_>) -> RunResult<Value> {
        match self {
            Self::Value(value) => Ok(value),
            // With no host to answer, `hasattr` and `getattr` see an undefined attribute.
            Self::AttrLookup {
                effect: Some(effect), ..
            } => Ok(effect.apply(None, vm)),
            Self::FramePushed => {
                // Stop both normal returns and exception unwinding before the Rust caller's frame.
                vm.current_frame_mut().should_return = true;
                loop {
                    match vm.run()? {
                        FrameExit::Return(value) => return Ok(value),
                        FrameExit::AttrLookup {
                            effect: Some(effect), ..
                        } => {
                            let value = effect.apply(None, vm);
                            vm.push(value);
                        }
                        exit => {
                            let error = vm.unsupported_frame_exit(exit);
                            if let Some(error) = vm.handle_exception(error) {
                                return Err(error);
                            }
                        }
                    }
                }
            }
            unsupported => Err(vm.unsupported_call_result(unsupported)),
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for CallResult {
    fn drop_with(self, heap: &mut C) {
        match self {
            Self::Value(value) | Self::AwaitValue(value) => value.drop_with(heap),
            Self::External { args, .. } | Self::MethodCall { args, .. } => {
                args.drop_with(heap);
            }
            Self::OsCall(call) => call.drop_with(heap),
            Self::AttrLookup { effect, .. } => effect.drop_with(heap),
            Self::FramePushed => {}
            Self::OsCallWithEffect { call, effect } => {
                call.drop_with(heap);
                // Discarded before it ever became a `FrameExit`.
                release_pending_effect(Some(effect), heap);
            }
        }
    }
}

impl<'h> VM<'h> {
    // ========================================================================
    // Call Opcode Executors
    // ========================================================================
    // These methods are called from the VM's main dispatch loop to execute
    // call-related opcodes. They handle stack operations and return a result
    // indicating what the VM should do next.

    /// Executes `CallFunction` opcode.
    ///
    /// Pops the callable and arguments from the stack, calls the function,
    /// and returns the result.
    pub(super) fn exec_call_function(&mut self, arg_count: usize) -> Result<CallResult, RunError> {
        let callable_index = self.stack.len() - arg_count - 1;
        let fast_call = match self.stack[callable_index] {
            Value::DefFunction(func_id) => self.try_call_exact_def_function(func_id, callable_index, arg_count),
            Value::Ref(id) => self.try_call_positional_defaults(id, callable_index, arg_count),
            _ => None,
        };
        if let Some(result) = fast_call {
            result
        } else {
            let args = self.pop_n_args(arg_count);
            let callable = self.pop();
            let this = self;
            defer_drop!(callable, this);
            this.call_function(callable, args)
        }
    }

    /// Copies just the missing defaults into the stack frame while the callable owns them.
    fn try_call_positional_defaults(
        &mut self,
        id: HeapId,
        callable_index: usize,
        arg_count: usize,
    ) -> Option<Result<CallResult, RunError>> {
        let HeapData::FunctionDefaults(function) = self.heap.get(id) else {
            return None;
        };
        let func_id = function.func_id;
        let func = self.interns.get_function(func_id);
        if func.is_async || !func.cell_var_slots.is_empty() || !func.free_var_slots.is_empty() {
            return None;
        }
        let defaults = func
            .signature
            .trailing_positional_defaults(arg_count, &function.defaults)?;
        let callable = self.stack.remove(callable_index);
        self.stack.reserve(func.namespace_size - arg_count);
        self.stack
            .extend(defaults.iter().map(|value| value.clone_with_heap(self.heap)));

        let frame_namespace = function_namespace(function.globals, self.heap);
        callable.drop_with(self);
        Some(self.push_stack_function(func_id, func, callable_index, frame_namespace))
    }

    /// Executes `CallBuiltinFunction` opcode.
    ///
    /// Calls a builtin function directly without stack manipulation for the callable.
    /// This is an optimization that avoids constant pool lookup and stack manipulation.
    pub(super) fn exec_call_builtin_function(
        &mut self,
        builtin_id: u8,
        arg_count: usize,
    ) -> Result<CallResult, RunError> {
        // Convert u8 to BuiltinsFunctions via FromRepr
        if let Some(builtin) = BuiltinsFunctions::from_repr(builtin_id) {
            let args = self.pop_n_args(arg_count);
            builtin.call(self, args)
        } else {
            Err(RunError::internal("CallBuiltinFunction: invalid builtin_id"))
        }
    }

    /// Executes `CallBuiltinType` opcode.
    ///
    /// Calls a builtin type constructor directly without stack manipulation for the callable.
    /// This is an optimization for type constructors like `list()`, `int()`, `str()`.
    pub(super) fn exec_call_builtin_type(&mut self, type_id: u8, arg_count: usize) -> Result<CallResult, RunError> {
        // Convert u8 to Type via callable_from_u8
        if let Some(t) = Type::callable_from_u8(type_id) {
            let args = self.pop_n_args(arg_count);
            t.call(self, args)
        } else {
            Err(RunError::internal("CallBuiltinType: invalid type_id"))
        }
    }

    /// Executes `CallFunctionKw` opcode.
    ///
    /// Binds eligible Python calls on the stack. Other calls use `ArgValues`.
    pub(super) fn exec_call_function_kw(&mut self, pos_count: usize, kw_count: usize) -> Result<CallResult, RunError> {
        if let Some(result) = self.try_call_direct_keywords(pos_count, kw_count) {
            return result;
        }

        // Pop keyword values (TOS is last kwarg value)
        let kw_values = self.pop_n(kw_count);

        // Pop positional arguments
        let pos_args = self.pop_n(pos_count);

        // Pop the callable
        let callable = self.pop();
        let this = self;
        defer_drop!(callable, this);

        // Build kwargs directly as Vec<(StringId, Value)>
        let mut kwargs_inline = Vec::with_capacity(kw_count);
        for value in kw_values {
            let name_id = StringId::from_index(this.current_frame.fetch_u16());
            kwargs_inline.push((name_id, value));
        }

        // Build ArgValues with both positional and keyword args
        let args = if pos_args.is_empty() && kwargs_inline.is_empty() {
            ArgValues::Empty
        } else if pos_args.is_empty() {
            ArgValues::Kwargs(KwargsValues::Inline(kwargs_inline))
        } else {
            ArgValues::ArgsKargs {
                args: pos_args,
                kwargs: KwargsValues::Inline(kwargs_inline),
            }
        };

        this.call_function(callable, args)
    }

    /// Reuses the argument stack slots as locals once all keywords are validated.
    fn try_call_direct_keywords(&mut self, pos_count: usize, kw_count: usize) -> Option<Result<CallResult, RunError>> {
        if kw_count == 0 || kw_count > Signature::MAX_DIRECT_KEYWORDS {
            return None;
        }
        let arg_count = pos_count + kw_count;
        let callable_index = self.stack.len() - arg_count - 1;
        // Leave the stack and instruction pointer intact for the general binder until every name matches.
        let ip = self.current_frame.ip;
        let end = ip + 2 * kw_count;
        let names = self.current_frame.bytecode.get(ip..end)?.as_chunks::<2>().0;
        let names = names
            .iter()
            .map(|&bytes| StringId::from_index(u16::from_le_bytes(bytes)));
        let func_id = match self.stack[callable_index] {
            Value::DefFunction(id) => id,
            Value::Ref(id) => return self.try_call_keyword_defaults(id, callable_index, pos_count, names, end),
            _ => return None,
        };
        let func = self.interns.get_function(func_id);
        if func.exact_positional_call() != Some(ExactPositionalCall::Sync(arg_count)) {
            return self.try_call_variadic_keywords(func_id, func, callable_index, pos_count, names, end);
        }
        let mut slots = [0; Signature::MAX_DIRECT_KEYWORDS];
        let destinations = &mut slots[..kw_count];
        func.signature.exact_keyword_slots(pos_count, names, destinations)?;

        // Each swap moves one value to its final parameter slot.
        let values = &mut self.stack[callable_index + 1 + pos_count..];
        for index in 0..kw_count {
            while usize::from(destinations[index]) != index {
                let destination = usize::from(destinations[index]);
                values.swap(index, destination);
                destinations.swap(index, destination);
            }
        }
        self.current_frame.ip = end;
        Some(self.call_exact_sync_function(func_id, func, callable_index))
    }

    /// Builds the capture dict and optional tuple directly from explicit arguments.
    fn try_call_variadic_keywords(
        &mut self,
        func_id: FunctionId,
        func: &'h Function,
        stack_base: usize,
        pos_count: usize,
        names: impl ExactSizeIterator<Item = StringId>,
        end: usize,
    ) -> Option<Result<CallResult, RunError>> {
        let (capture_args, capture_kwargs) = func.signature.variadic_only()?;
        if !capture_kwargs
            || (!capture_args && pos_count != 0)
            || func.is_async
            || !func.cell_var_slots.is_empty()
            || !func.free_var_slots.is_empty()
        {
            return None;
        }
        let keyword_base = stack_base + 1 + pos_count;
        let kwargs = Dict::from_explicit_keywords(names.zip(self.stack.drain(keyword_base..)), self.interns);
        let items = self.stack.drain(stack_base + 1..).collect();
        let callable = self.pop();
        debug_assert_exact_callable(&callable, func_id);
        if capture_args {
            let args = allocate_tuple(items, self.heap);
            self.push(args);
        } else {
            debug_assert!(items.is_empty());
        }
        let kwargs = Value::Ref(self.heap.allocate(HeapData::Dict(kwargs)));
        self.push(kwargs);
        self.current_frame.ip = end;
        Some(self.push_stack_function(func_id, func, stack_base, None))
    }

    /// Binds explicit keywords and fills any unoccupied slots from the callable's defaults.
    fn try_call_keyword_defaults(
        &mut self,
        id: HeapId,
        callable_index: usize,
        pos_count: usize,
        names: impl ExactSizeIterator<Item = StringId>,
        end: usize,
    ) -> Option<Result<CallResult, RunError>> {
        const EMPTY: u8 = u8::MAX;

        let HeapData::FunctionDefaults(function) = self.heap.get(id) else {
            return None;
        };
        let func_id = function.func_id;
        let func = self.interns.get_function(func_id);
        if func.is_async || !func.cell_var_slots.is_empty() || !func.free_var_slots.is_empty() {
            return None;
        }

        let kw_count = names.len();
        let mut destinations = [EMPTY; Signature::MAX_DIRECT_KEYWORDS];
        let (remaining, missing) =
            func.signature
                .keyword_default_slots(pos_count, names, &mut destinations[..kw_count], |name, param| {
                    *name == param
                })?;

        let callable = self.stack.remove(callable_index);
        let keyword_base = callable_index + pos_count;
        self.stack.resize_with(keyword_base + remaining, || Value::Undefined);
        let values = &mut self.stack[keyword_base..];

        // Swapping into an empty slot ends a chain. Cycles end when the starting slot is filled.
        for index in 0..kw_count {
            let mut destination = destinations[index];
            while destination != EMPTY && usize::from(destination) != index {
                let slot = usize::from(destination);
                values.swap(index, slot);
                destinations.swap(index, slot);
                destination = destinations[index];
            }
        }
        fill_default_slots(values, &function.defaults, missing, self.heap);

        let frame_namespace = function_namespace(function.globals, self.heap);
        callable.drop_with(self);
        self.current_frame.ip = end;
        Some(self.push_stack_function(func_id, func, callable_index, frame_namespace))
    }

    /// Executes `CallAttr` opcode.
    ///
    /// Pops the object and arguments from the stack, calls the attribute,
    /// and returns a `CallResult` which may indicate an OS or external call.
    pub(super) fn exec_call_attr(&mut self, name_id: StringId, arg_count: usize) -> Result<CallResult, RunError> {
        let args = self.pop_n_args(arg_count);
        let obj = self.pop();
        self.call_attr(obj, name_id, args)
    }

    /// Executes `CallAttrKw` opcode.
    ///
    /// Reads `kw_count` keyword name operands from the instruction stream,
    /// pops the keyword values, positional args, and object from the stack,
    /// builds the appropriate `ArgValues`, and calls the attribute.
    /// Returns a `CallResult` which may indicate an OS or external call.
    pub(super) fn exec_call_attr_kw(
        &mut self,
        name_id: StringId,
        pos_count: usize,
        kw_count: usize,
    ) -> Result<CallResult, RunError> {
        // Pop keyword values (TOS is last kwarg value)
        let kw_values = self.pop_n(kw_count);

        // Pop positional arguments
        let pos_args = self.pop_n(pos_count);

        // Pop the object
        let obj = self.pop();

        // Build kwargs directly as Vec<(StringId, Value)>
        let mut kwargs_inline = Vec::with_capacity(kw_count);
        for value in kw_values {
            let kw_name_id = StringId::from_index(self.current_frame.fetch_u16());
            kwargs_inline.push((kw_name_id, value));
        }

        // Build ArgValues with both positional and keyword args
        let args = if pos_args.is_empty() && kwargs_inline.is_empty() {
            ArgValues::Empty
        } else if pos_args.is_empty() {
            ArgValues::Kwargs(KwargsValues::Inline(kwargs_inline))
        } else {
            ArgValues::ArgsKargs {
                args: pos_args,
                kwargs: KwargsValues::Inline(kwargs_inline),
            }
        };

        self.call_attr(obj, name_id, args)
    }

    /// Finishes a positional unpacked call directly from list or tuple storage.
    /// The last unpack must be immediately followed by the call's tuple conversion
    /// and call, so no later argument evaluation can mutate the source before copying.
    pub(super) fn try_call_unpacked_sequence(&mut self) -> Option<Result<CallResult, RunError>> {
        let ip = self.current_frame.ip;
        if !self.current_frame.bytecode.get(ip..)?.starts_with(&[
            Opcode::ListToTuple as u8,
            Opcode::CallFunctionExtended as u8,
            0,
        ]) {
            return None;
        }
        let [_, Value::Ref(prefix_id), Value::Ref(source_id)] = self.stack.last_chunk::<3>()? else {
            return None;
        };
        let HeapData::List(prefix) = self.heap.get(*prefix_id) else {
            return None;
        };
        let source = match self.heap.get(*source_id) {
            HeapData::List(list) => list.as_slice(),
            HeapData::Tuple(tuple) => tuple.as_slice(),
            _ => return None,
        };
        let arg_count = prefix.len() + source.len();
        // Larger unpackings use the iterator's cooperative resource checks.
        if arg_count > usize::from(u8::MAX) {
            return None;
        }
        if let Err(error) = self.check_argument_stack_growth(self.stack.len() - 2 + arg_count) {
            return Some(Err(error));
        }

        let source_owner = self.stack.pop().expect("unpack source");
        let prefix_owner = self.stack.pop().expect("unpack prefix");
        self.stack.extend(
            prefix
                .as_slice()
                .iter()
                .chain(source)
                .map(|item| item.clone_with_heap(self.heap)),
        );
        (source_owner, prefix_owner).drop_with(self);

        self.instruction_ip = ip + 1;
        self.current_frame.ip = ip + 3;
        Some(self.exec_call_function(arg_count))
    }

    /// Binds a final `**dict` directly when the merge has no earlier keywords.
    /// Validation leaves the inputs intact so the general path reports binding and merge errors.
    pub(super) fn try_call_unpacked_keywords(&mut self) -> Option<Result<CallResult, RunError>> {
        let ip = self.current_frame.ip;
        if !self
            .current_frame
            .bytecode
            .get(ip..)?
            .starts_with(&[Opcode::CallFunctionExtended as u8, 1])
        {
            return None;
        }
        let [callable, Value::Ref(args), Value::Ref(merged), Value::Ref(source)] = self.stack.last_chunk::<4>()? else {
            return None;
        };
        let HeapData::Dict(merged) = self.heap.get(*merged) else {
            return None;
        };
        let HeapData::Dict(source) = self.heap.get(*source) else {
            return None;
        };
        let kw_count = source.len();
        if !merged.is_empty() || kw_count > Signature::MAX_DIRECT_KEYWORDS {
            return None;
        }
        let HeapData::Tuple(args) = self.heap.get(*args) else {
            return None;
        };
        let pos_args = args.as_slice();
        if pos_args.len() > usize::from(u8::MAX) {
            return None;
        }
        let (func_id, defaults, globals) = match callable {
            Value::DefFunction(id) => (*id, &[][..], None),
            Value::Ref(id) => match self.heap.get(*id) {
                HeapData::FunctionDefaults(function) => {
                    (function.func_id, function.defaults.as_slice(), function.globals)
                }
                _ => return None,
            },
            _ => return None,
        };
        let func = self.interns.get_function(func_id);
        if func.is_async || !func.cell_var_slots.is_empty() || !func.free_var_slots.is_empty() {
            return None;
        }

        let pairs = source.dense_pairs()?;
        let keys = pairs.clone().map(|(key, _)| key.as_str_heap(self.heap, self.interns));
        let mut slots = [0; Signature::MAX_DIRECT_KEYWORDS];
        let destinations = &mut slots[..kw_count];
        let (remaining, missing) =
            func.signature
                .keyword_default_slots(pos_args.len(), keys, destinations, |key, param| {
                    *key == Some(self.interns.get_str(param))
                })?;
        let stack_base = self.stack.len() - 4;
        if let Err(error) = self.check_argument_stack_growth(stack_base + func.namespace_size) {
            return Some(Err(error));
        }

        let owners: [Value; 4] = array::from_fn(|_| self.stack.pop().expect("unpacked call operand"));
        self.stack
            .extend(pos_args.iter().map(|value| value.clone_with_heap(self.heap)));
        let keyword_base = self.stack.len();
        self.stack.resize_with(keyword_base + remaining, || Value::Undefined);
        let values = &mut self.stack[keyword_base..];
        for ((_, value), &mut slot) in pairs.zip(destinations) {
            values[usize::from(slot)] = value.clone_with_heap(self.heap);
        }
        fill_default_slots(values, defaults, missing, self.heap);
        let frame_namespace = function_namespace(globals, self.heap);
        owners.drop_with(self);

        self.instruction_ip = ip;
        self.current_frame.ip = ip + 2;
        Some(self.push_stack_function(func_id, func, stack_base, frame_namespace))
    }

    fn check_argument_stack_growth(&self, required: usize) -> RunResult<()> {
        let capacity = self.stack.capacity();
        if required > capacity {
            let capacity = capacity.saturating_mul(2).max(required);
            check_estimated_size(capacity.saturating_mul(VALUE_SIZE), &self.heap.tracker)?;
        }
        Ok(())
    }

    /// Executes `CallFunctionExtended` opcode.
    ///
    /// Handles calls with `*args` and/or `**kwargs` unpacking.
    pub(super) fn exec_call_function_extended(&mut self, has_kwargs: bool) -> Result<CallResult, RunError> {
        if let Some(result) = self.try_call_variadic_extended(has_kwargs) {
            return result;
        }

        // Pop kwargs dict if present
        let kwargs = if has_kwargs { Some(self.pop()) } else { None };

        // Pop args tuple
        let args_tuple = self.pop();

        // Pop callable
        let callable = self.pop();

        // Unpack and call
        self.call_function_extended(callable, args_tuple, kwargs)
    }

    /// Installs the private unpacking containers directly for a variadic-only signature.
    fn try_call_variadic_extended(&mut self, has_kwargs: bool) -> Option<Result<CallResult, RunError>> {
        let stack_base = self.stack.len() - 2 - usize::from(has_kwargs);
        let Value::DefFunction(func_id) = self.stack[stack_base] else {
            return None;
        };
        let func = self.interns.get_function(func_id);
        let (capture_args, capture_kwargs) = func.signature.variadic_only()?;
        if func.is_async || !func.cell_var_slots.is_empty() || !func.free_var_slots.is_empty() {
            return None;
        }
        if !capture_args {
            let Value::Ref(id) = self.stack[stack_base + 1] else {
                return None;
            };
            let HeapData::Tuple(args) = self.heap.get(id) else {
                return None;
            };
            if !args.as_slice().is_empty() {
                return None;
            }
        }
        if has_kwargs && !capture_kwargs {
            let Value::Ref(id) = self.stack[stack_base + 2] else {
                return None;
            };
            let HeapData::Dict(kwargs) = self.heap.get(id) else {
                return None;
            };
            if !kwargs.is_empty() {
                return None;
            }
        }

        let kwargs = has_kwargs.then(|| self.pop());
        let args = self.pop();
        let callable = self.pop();
        debug_assert_exact_callable(&callable, func_id);
        if capture_args {
            self.push(args);
        } else {
            args.drop_with(self);
        }
        if capture_kwargs {
            let kwargs = kwargs.unwrap_or_else(|| Value::Ref(self.heap.allocate(HeapData::Dict(Dict::new()))));
            self.push(kwargs);
        } else {
            kwargs.drop_with(self);
        }
        Some(self.push_stack_function(func_id, func, stack_base, None))
    }

    /// Executes `CallAttrExtended` opcode.
    ///
    /// Handles method calls with `*args` and/or `**kwargs` unpacking.
    pub(super) fn exec_call_attr_extended(
        &mut self,
        name_id: StringId,
        has_kwargs: bool,
    ) -> Result<CallResult, RunError> {
        // Pop kwargs dict if present
        let kwargs = if has_kwargs { Some(self.pop()) } else { None };

        // Pop args tuple
        let args_tuple = self.pop();

        // Pop the receiver object
        let obj = self.pop();

        // Unpack and call
        self.call_attr_extended(obj, name_id, args_tuple, kwargs)
    }

    // ========================================================================
    // Internal Call Helpers
    // ========================================================================

    /// Pops n arguments from the stack and wraps them in `ArgValues`.
    fn pop_n_args(&mut self, n: usize) -> ArgValues {
        match n {
            0 => ArgValues::Empty,
            1 => ArgValues::One(self.pop()),
            2 => {
                let b = self.pop();
                let a = self.pop();
                ArgValues::Two(a, b)
            }
            _ => ArgValues::ArgsKargs {
                args: self.pop_n(n),
                kwargs: KwargsValues::Empty,
            },
        }
    }

    /// Calls an attribute on an object.
    ///
    /// Logical dispatch hides interned versus heap storage and preserves suspendable
    /// results for methods requiring host involvement.
    ///
    /// **Dunder dispatch**: before reaching the type-specific dispatcher, this
    /// method intercepts known dunder names (`__enter__`, `__exit__`, …) and
    /// routes them to the corresponding [`PyTrait`] method
    /// (`py_enter` / `py_exit` / …). The default trait impls return
    /// `AttributeError`, so types that don't override the dunder behave
    /// identically to a generic "no such method" lookup; types that *do*
    /// override only need a single trait impl, not parallel `StaticStrings::Foo`
    /// arms in their `py_call_attr` body. New dunder methods plug into the
    /// dispatch table here without touching individual types.
    pub(crate) fn call_attr(&mut self, obj: Value, name_id: StringId, args: ArgValues) -> Result<CallResult, RunError> {
        let this = self;
        let attr = EitherStr::Interned(name_id);

        // Centralised dunder dispatch — see `dispatch_dunder`. Wrap `args`
        // in an `Option` so the helper can `take()` it only when it
        // actually matches a dunder; on the fall-through path the
        // original `args` is still owned here and goes into `py_call_attr`.
        let mut args_slot = Some(args);
        if let Value::Ref(heap_id) = obj
            && let Some(result) = dispatch_dunder(name_id, heap_id, this, &mut args_slot)
        {
            defer_drop!(obj, this);
            return result;
        }
        let args = args_slot.expect("dispatch_dunder returned None without taking args");

        defer_drop!(obj, this);
        obj.py_value(this).py_call_attr(this, &attr, args)
    }

    /// Evaluates a function in a synchronous position that cannot suspend to the host.
    ///
    /// User-defined functions run until their frame returns. An unresolved name raises
    /// `NameError`; external, OS, method, and future suspensions raise a variant-specific
    /// `NotImplementedError` because this context cannot preserve and resume its state.
    /// These are raised at the suspension point inside the callee's frames, so the
    /// callee can catch them with an ordinary `try`/`except` like any other exception.
    ///
    /// The nested `self.run()` below recurses on the native Rust stack, so
    /// re-entry is bounded via [`enter_run_reentry`](Self::enter_run_reentry)
    /// at entry — before `call_function`, since a class-valued `__init__` can
    /// recurse back in without ever pushing a frame.
    pub(crate) fn evaluate_function(&mut self, callable: &Value, args: ArgValues) -> Result<Value, RunError> {
        if let Err(e) = self.enter_run_reentry() {
            // Bailing before `call_function` takes ownership of `args`, so
            // reclaim its refcounts here.
            args.drop_with(self);
            return Err(e.into());
        }
        let mut guard = RunReentryGuard::new(self);
        let this = &mut *guard;

        this.call_function(callable, args)?.finish_synchronously(this)
    }

    /// Converts a direct call suspension into a specific synchronous-context error.
    #[cold]
    fn unsupported_call_result(&mut self, result: CallResult) -> RunError {
        let error = match &result {
            CallResult::External {
                name: function_name, ..
            } => ExcType::not_implemented_synchronous_call("external function", function_name.as_str(self.interns)),
            CallResult::OsCall(call) | CallResult::OsCallWithEffect { call, .. } => {
                ExcType::not_implemented_synchronous_call("OS function", call.name())
            }
            CallResult::MethodCall { name, .. } => {
                ExcType::not_implemented_synchronous_call("method call", name.as_str(self.interns))
            }
            // A lazy attribute lookup cannot suspend in a synchronous nested
            // context; report the attribute as missing rather than a
            // NotImplementedError, matching what an unanswered lookup raises
            // (an `effect` was consumed by the caller before reaching here).
            CallResult::AttrLookup {
                name,
                class_name,
                type_object,
                ..
            } => {
                let error = if *type_object {
                    ExcType::attribute_error_type(class_name, name.as_str(self.interns))
                } else {
                    ExcType::attribute_error(class_name, name.as_str(self.interns))
                };
                result.drop_with(self);
                return error;
            }
            CallResult::AwaitValue(_) => ExcType::not_implemented("cannot await a value in this synchronous context"),
            CallResult::Value(_) | CallResult::FramePushed => unreachable!("completed calls are handled above"),
        };
        result.drop_with(self);
        error.into()
    }

    /// Converts a nested VM suspension into a specific synchronous-context error.
    #[cold]
    pub(crate) fn unsupported_frame_exit(&mut self, exit: FrameExit) -> RunError {
        let error = match &exit {
            FrameExit::Return(_) => unreachable!("return exits are handled above"),
            FrameExit::ExternalCall { function_name, .. } => {
                ExcType::not_implemented_synchronous_call("external function", function_name.as_str(self.interns))
            }
            FrameExit::OsCall { function_call, .. } => {
                ExcType::not_implemented_synchronous_call("OS function", function_call.name())
            }
            FrameExit::MethodCall { method_name, .. } => {
                ExcType::not_implemented_synchronous_call("method call", method_name.as_str(self.interns))
            }
            // Same as the `CallResult::AttrLookup` arm above.
            FrameExit::AttrLookup {
                name,
                class_name,
                type_object,
                ..
            } => {
                let error = if *type_object {
                    ExcType::attribute_error_type(class_name, name.as_str(self.interns))
                } else {
                    ExcType::attribute_error(class_name, name.as_str(self.interns))
                };
                exit.drop_with(self);
                return error;
            }
            FrameExit::ResolveFutures(_) => {
                ExcType::not_implemented("cannot resolve async futures in this synchronous context")
            }
            FrameExit::NameLookup { name_id, .. } => ExcType::name_error(self.interns.get_str(*name_id)),
        };
        exit.drop_with(self);
        error.into()
    }

    /// Calls a callable value with the given arguments.
    ///
    /// Dispatches based on the callable type:
    /// - `Value::Builtin`: calls builtin directly, returns `Push`
    /// - `Value::ModuleFunction`: calls module function directly, returns `Push`
    /// - `Value::DefFunction`: pushes a new frame, returns `FramePushed`
    /// - `Value::Ref`: checks for closure/function on heap
    pub(crate) fn call_function(&mut self, callable: &Value, args: ArgValues) -> Result<CallResult, RunError> {
        match callable {
            Value::Builtin(builtin) => builtin.call(self, args),
            Value::ModuleFunction(mf) => mf.call(self, args),
            Value::DefFunction(func_id) => {
                // Defined function without defaults, captured variables or a globals dict
                self.call_def_function(*func_id, &[], &[], None, args)
            }
            Value::Ref(heap_id) => {
                // Could be a closure or function with defaults - check heap
                self.call_heap_callable(*heap_id, args)
            }
            _ => {
                // Coupling check: reaching here means dispatch rejected the value,
                // so `Value::is_callable` must agree it is not callable.
                debug_assert!(
                    !callable.is_callable(self),
                    "Value::is_callable accepts a value call_function rejects — the two drifted"
                );
                args.drop_with(self);
                let ty = callable.py_type_name(self);
                Err(ExcType::type_error(format!("'{ty}' object is not callable")))
            }
        }
    }

    /// Calls a heap value, delegating to the type's own [`PyTrait::py_call`].
    ///
    /// Each callable type owns its dispatch, so this only reads the value and
    /// hands the arguments over — a new callable type is added by implementing
    /// `py_call`, not by extending anything here. The default `py_call` raises
    /// the same `TypeError` the rejected arm used to, so a value that is not
    /// callable is refused without a branch of its own.
    fn call_heap_callable(&mut self, heap_id: HeapId, args: ArgValues) -> Result<CallResult, RunError> {
        PyValue::from_heap(heap_id, self.heap).py_call(args, self)
    }

    /// Calls a function with unpacked args tuple and optional kwargs dict.
    ///
    /// Used for `f(*args)` and `f(**kwargs)` style calls.
    fn call_function_extended(
        &mut self,
        callable: Value,
        args_tuple: Value,
        kwargs: Option<Value>,
    ) -> Result<CallResult, RunError> {
        let this = self;
        defer_drop!(args_tuple, this);
        defer_drop!(callable, this);
        let args = this.take_extended_args(args_tuple, kwargs)?;
        this.call_function(callable, args)
    }

    /// Calls a method with unpacked args tuple and optional kwargs dict.
    ///
    /// Used for `obj.method(*args)` and `obj.method(**kwargs)` style calls.
    fn call_attr_extended(
        &mut self,
        obj: Value,
        name_id: StringId,
        args_tuple: Value,
        kwargs: Option<Value>,
    ) -> Result<CallResult, RunError> {
        let this = self;
        defer_drop!(args_tuple, this);
        let mut pending = DropGuard::new(obj, this);
        let args = pending.ctx().take_extended_args(args_tuple, kwargs)?;
        let (obj, this) = pending.into_parts();
        this.call_attr(obj, name_id, args)
    }

    /// Takes the contents of the compiler's argument containers. Non-empty positional tuples
    /// and keyword dicts are private to the call, so binding can take their owned values.
    fn take_extended_args(&mut self, args_tuple: &Value, kwargs: Option<Value>) -> RunResult<ArgValues> {
        let mut pending = DropGuard::new(kwargs, self);
        let items = pending.ctx().take_extended_positional(args_tuple)?;
        let (kwargs, this) = pending.into_parts();
        let kwargs = kwargs.map_or(KwargsValues::Empty, |value| this.take_extended_keywords(value));
        Ok(match (items.len(), kwargs) {
            (0, KwargsValues::Empty) => ArgValues::Empty,
            (1, KwargsValues::Empty) => ArgValues::One(items.into_iter().next().unwrap()),
            (2, KwargsValues::Empty) => {
                let mut items = items.into_iter();
                ArgValues::Two(items.next().unwrap(), items.next().unwrap())
            }
            (0, kwargs) => ArgValues::Kwargs(kwargs),
            (_, kwargs) => {
                let mut args = items.into_vec();
                // A bound method may prepend its receiver.
                args.reserve_exact(1);
                ArgValues::ArgsKargs { args, kwargs }
            }
        })
    }

    fn take_extended_positional(&mut self, args_tuple: &Value) -> RunResult<TupleVec> {
        let Value::Ref(id) = args_tuple else {
            unreachable!("unpacked positionals must be a tuple")
        };
        let tuple = self
            .heap
            .read_as::<Tuple>(*id)
            .expect("unpacked positionals must be a tuple");
        let len = tuple.get(self.heap).as_slice().len();
        if len == 0 {
            // The empty tuple is shared.
            return Ok(TupleVec::new());
        }
        check_estimated_size(len.saturating_add(1).saturating_mul(VALUE_SIZE), &self.heap.tracker)?;
        Ok(tuple.get_mut(self.heap).take_items())
    }

    fn take_extended_keywords(&mut self, kwargs: Value) -> KwargsValues {
        let this = self;
        defer_drop!(kwargs, this);
        let Value::Ref(id) = kwargs else {
            unreachable!("unpacked keywords must be a dict")
        };
        let dict = this
            .heap
            .read_as::<Dict>(*id)
            .expect("unpacked keywords must be a dict");
        if dict.get(this.heap).is_empty() {
            KwargsValues::Empty
        } else {
            KwargsValues::Dict(mem::take(dict.get_mut(this.heap)))
        }
    }

    // ========================================================================
    // Frame Setup
    // ========================================================================

    /// Bypasses argument binding when a direct call matches its cached call plan.
    fn try_call_exact_def_function(
        &mut self,
        func_id: FunctionId,
        callable_index: usize,
        arg_count: usize,
    ) -> Option<Result<CallResult, RunError>> {
        let func = self.interns.get_function(func_id);
        match func.exact_positional_call() {
            Some(ExactPositionalCall::Sync(count)) if count == arg_count => {
                Some(self.call_exact_sync_function(func_id, func, callable_index))
            }
            Some(ExactPositionalCall::Async(count)) if count == arg_count => {
                Some(Ok(self.create_exact_coroutine(func_id, func, callable_index)))
            }
            _ => self.try_call_variadic_positional(func_id, func, callable_index, arg_count),
        }
    }

    /// Moves the positional tail directly into the callee's `*args` tuple.
    fn try_call_variadic_positional(
        &mut self,
        func_id: FunctionId,
        func: &'h Function,
        callable_index: usize,
        arg_count: usize,
    ) -> Option<Result<CallResult, RunError>> {
        let (capture_args, capture_kwargs) = func.signature.variadic_only()?;
        if func.is_async
            || !func.cell_var_slots.is_empty()
            || !func.free_var_slots.is_empty()
            || (!capture_args && arg_count != 0)
        {
            return None;
        }
        let slots = arg_count.checked_next_power_of_two().unwrap_or(usize::MAX);
        if let Err(error) = check_estimated_size(slots.saturating_mul(VALUE_SIZE), &self.heap.tracker) {
            return Some(Err(error.into()));
        }

        let items = self.stack.drain(callable_index + 1..).collect();
        let callable = self.pop();
        debug_assert_exact_callable(&callable, func_id);
        if capture_args {
            let args = allocate_tuple(items, self.heap);
            self.push(args);
        } else {
            debug_assert!(items.is_empty());
        }
        if capture_kwargs {
            let kwargs = Value::Ref(self.heap.allocate(HeapData::Dict(Dict::new())));
            self.push(kwargs);
        }
        Some(self.push_stack_function(func_id, func, callable_index, None))
    }

    /// Pushes a simple function frame using arguments already present on the VM stack.
    ///
    /// Removing the immediate callable shifts its arguments into the parameter
    /// slots, avoiding argument wrappers, binding, scratch storage, and a copy.
    fn call_exact_sync_function(
        &mut self,
        func_id: FunctionId,
        func: &'h Function,
        callable_index: usize,
    ) -> Result<CallResult, RunError> {
        let callable = self.stack.remove(callable_index);
        debug_assert_exact_callable(&callable, func_id);
        self.push_stack_function(func_id, func, callable_index, None)
    }

    /// Extends the stack's bound argument slots to hold the remaining locals.
    fn push_stack_function(
        &mut self,
        func_id: FunctionId,
        func: &'h Function,
        stack_base: usize,
        frame_namespace: Option<Box<FrameNamespace>>,
    ) -> Result<CallResult, RunError> {
        let call_offset = self.current_offset();
        let namespace_size = func.namespace_size;
        let locals_count = u16::try_from(namespace_size).expect("function namespace size exceeds u16");
        self.stack.resize_with(stack_base + namespace_size, || Value::Undefined);

        let exc_stack_base = self.exception_stack.len();
        self.push_frame(CallFrame::new_function(
            &func.code,
            stack_base,
            locals_count,
            exc_stack_base,
            func_id,
            call_offset,
            frame_namespace,
        ))?;

        Ok(CallResult::FramePushed)
    }

    /// Creates a coroutine by moving exact positional arguments into its namespace.
    fn create_exact_coroutine(&mut self, func_id: FunctionId, func: &Function, callable_index: usize) -> CallResult {
        let mut namespace = self.stack.split_off(callable_index + 1);
        namespace.reserve(func.namespace_size - namespace.len());
        self.install_closure_cells(func, &[], &mut namespace);

        let callable = self.pop();
        debug_assert_exact_callable(&callable, func_id);

        let coroutine = Coroutine::new(func_id, namespace, None);
        let coroutine_id = self.heap.allocate(HeapData::Coroutine(coroutine));
        CallResult::Value(Value::Ref(coroutine_id))
    }

    /// Calls an internal function, compiling it on first use.
    /// Arguments transfer into ordinary frame locals; implementation frames do not charge recursion depth.
    pub(crate) fn call_frozen_exact<const N: usize>(
        &mut self,
        function: FrozenFunction,
        args: [Value; N],
    ) -> Result<CallResult, RunError> {
        let mut args_guard = DropGuard::new(args, self);
        let (_, vm) = args_guard.as_parts_mut();
        let interns = vm.interns;
        let (function_id, overlay) = if let Some(id) = interns.frozen_function(function) {
            (id, None)
        } else {
            let (id, overlay) = function.compile(interns)?;
            (id, Some(overlay))
        };
        // No fallible operation or Python execution may intervene between publication and frame installation.
        if let Some(overlay) = overlay {
            overlay.commit();
            interns.set_frozen_function(function, function_id);
        }
        let (args, vm) = args_guard.into_parts();
        let func = interns.get_function(function_id);
        assert_eq!(func.signature.exact_positional_count(), Some(N));
        assert!(func.cell_var_slots.is_empty() && func.free_var_slots.is_empty());
        let namespace_size = func.namespace_size;
        let locals_count = u16::try_from(namespace_size).expect("frozen function namespace exceeds u16");
        assert!(namespace_size >= N, "frozen function has too few local slots");
        let stack_base = vm.stack.len();
        vm.stack.extend(args);
        vm.stack.resize_with(stack_base + namespace_size, || Value::Undefined);
        let frame = CallFrame::new_function(
            &func.code,
            stack_base,
            locals_count,
            vm.exception_stack.len(),
            function_id,
            vm.current_offset(),
            None,
        );
        vm.push_admitted_frame(frame);

        Ok(CallResult::FramePushed)
    }

    /// Calls a defined function by pushing a new frame or creating a coroutine.
    ///
    /// For sync functions: sets up the function's namespace with bound arguments,
    /// cell variables, and free variables, then pushes a new frame.
    ///
    /// For async functions: binds arguments immediately but returns a Coroutine
    /// instead of pushing a frame. The coroutine stores the pre-bound namespace
    /// and will be executed when awaited.
    pub(crate) fn call_def_function(
        &mut self,
        func_id: FunctionId,
        cells: &[HeapId],
        defaults: &[Value],
        globals: Option<HeapId>,
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        let func = self.interns.get_function(func_id);

        if func.is_async {
            self.create_coroutine(func_id, func, cells, defaults, globals, args)
        } else {
            let namespace = function_namespace(globals, &*self.heap);
            self.call_sync_function(func_id, func, cells, defaults, namespace, args)
        }
    }

    /// Calls an `__annotate__` function with `format` (PEP 649).
    ///
    /// A `def` runs under [`FrameNamespace::Annotate`]: names resolve through
    /// `classdict` (a borrowed class-namespace copy, for a class's annotations),
    /// then its globals, with `missing` standing in for CPython's "fake globals"
    /// in `FORWARDREF` and `STRING` format. Any other callable runs plainly.
    /// Takes ownership of `missing`.
    pub(crate) fn call_annotate(
        &mut self,
        annotate: &Value,
        format: i64,
        classdict: Option<HeapId>,
        missing: MissingNames,
    ) -> Result<CallResult, RunError> {
        let this = self;
        let args = ArgValues::One(Value::Int(format));
        // The guard releases `missing` on the paths that run without a namespace.
        let mut missing_guard = DropGuard::new(missing, this);
        let (_, this) = missing_guard.as_parts_mut();
        let Some(FunctionParts {
            func_id,
            cells,
            defaults,
            globals,
        }) = this.function_parts(annotate)
        else {
            return this.call_function(annotate, args);
        };
        let func = this.interns.get_function(func_id);
        if func.is_async {
            defer_drop!(defaults, this);
            return this.create_coroutine(func_id, func, &cells, defaults, globals, args);
        }
        let (missing, this) = missing_guard.into_parts();
        defer_drop!(defaults, this);
        let globals = match globals {
            Some(dict) => {
                this.heap.inc_ref(dict);
                FrameGlobals::Dict(dict)
            }
            None => FrameGlobals::Slots,
        };
        if let Some(dict) = classdict {
            this.heap.inc_ref(dict);
        }
        let namespace = Box::new(FrameNamespace::Annotate {
            globals,
            classdict,
            missing,
        });
        this.call_sync_function(func_id, func, &cells, defaults, Some(namespace), args)
    }

    /// The [`FunctionParts`] of a Python function value; `None` for a callable
    /// that is not a `def` (a builtin, a bound method, ...).
    fn function_parts(&self, value: &Value) -> Option<FunctionParts> {
        let parts = |func_id, cells, defaults: &[Value], globals| FunctionParts {
            func_id,
            cells,
            defaults: defaults.iter().map(|v| v.clone_with_heap(self)).collect(),
            globals,
        };
        match value {
            Value::DefFunction(func_id) => Some(parts(*func_id, Box::default(), &[], None)),
            Value::Ref(id) => match self.heap.get(*id) {
                HeapData::FunctionDefaults(function) => Some(parts(
                    function.func_id,
                    Box::default(),
                    &function.defaults,
                    function.globals,
                )),
                HeapData::Closure(closure) => Some(parts(
                    closure.func_id,
                    closure.cells.clone(),
                    &closure.defaults,
                    closure.globals,
                )),
                _ => None,
            },
            _ => None,
        }
    }

    /// Creates a Coroutine for an async function call.
    ///
    /// The coroutine is executed when awaited via Await.
    fn create_coroutine(
        &mut self,
        func_id: FunctionId,
        func: &Function,
        cells: &[HeapId],
        defaults: &[Value],
        globals: Option<HeapId>,
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        // 1. Create namespace for the coroutine with bound arguments and captured cells.
        let namespace = Vec::with_capacity(func.namespace_size);
        let mut namespace_guard = DropGuard::new(namespace, self);
        let (namespace, this) = namespace_guard.as_parts_mut();

        // 2. Bind arguments to parameters
        func.signature.bind(args, defaults, this, func.name, namespace)?;

        // 3. Install owned cells and captured free-var cells at their slots.
        this.install_closure_cells(func, cells, namespace);

        // 4. Create Coroutine on heap; it carries its own reference to the globals dict.
        let (namespace, this) = namespace_guard.into_parts();
        if let Some(globals) = globals {
            this.heap.inc_ref(globals);
        }
        let coroutine = Coroutine::new(func_id, namespace, globals);
        let coroutine_id = this.heap.allocate(HeapData::Coroutine(coroutine));

        Ok(CallResult::Value(Value::Ref(coroutine_id)))
    }

    /// Installs owned cell variables and captured free-var cells into a frame's
    /// `namespace` at their explicit slots, then fills any remaining slots with
    /// `Undefined`.
    ///
    /// `namespace` enters holding only the bound parameters and leaves with
    /// length `func.namespace_size`. Each owned cell (`cell_var_slots[i]`) is a
    /// freshly allocated `Cell`, seeded from parameter `cell_param_indices[i]`
    /// when that cell is for a captured parameter. Each captured cell
    /// (`cells[i]`, gathered by the caller from the enclosing frame) is inc-ref'd
    /// and installed at `free_var_slots[i]`.
    ///
    /// Slots are addressed explicitly rather than pushed sequentially because a
    /// transitively captured (pass-through) variable is allocated a slot late
    /// during preparation, outside the contiguous param/cell/free region — so a
    /// positional `push` would place it wrong. Shared by sync calls and
    /// coroutine creation, including the exact-positional-call fast path
    /// (which always passes an empty `cells` slice, since it only applies when
    /// `cell_var_slots`/`free_var_slots` are both empty).
    fn install_closure_cells(&mut self, func: &Function, cells: &[HeapId], namespace: &mut Vec<Value>) {
        namespace.resize_with(func.namespace_size, || Value::Undefined);

        for (i, &slot) in func.cell_var_slots.iter().enumerate() {
            let cell_value = match func.cell_param_indices[i] {
                Some(param_idx) => namespace[param_idx].clone_with_heap(self),
                None => Value::Undefined,
            };
            let cell_id = self.heap.allocate(HeapData::Cell(CellValue(cell_value)));
            namespace[slot.index()] = Value::Ref(cell_id);
        }

        for (i, &cell_id) in cells.iter().enumerate() {
            self.heap.inc_ref(cell_id);
            namespace[func.free_var_slots[i].index()] = Value::Ref(cell_id);
        }
    }

    /// Calls a sync function by pushing a new frame.
    ///
    /// Sets up the function's namespace with bound arguments, cell variables,
    /// and free variables (captured from enclosing scope for closures), and
    /// installs `frame_namespace` (owned; released here if binding fails).
    ///
    /// Locals are built in the reusable `namespace_scratch` buffer (under a
    /// [`DropGuard`] for cleanup on error) and moved onto the VM stack, where
    /// `stack_base` points to the start of the locals region.
    fn call_sync_function(
        &mut self,
        func_id: FunctionId,
        func: &'h Function,
        cells: &[HeapId],
        defaults: &[Value],
        frame_namespace: Option<Box<FrameNamespace>>,
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        let call_offset = self.current_offset();
        let stack_base = self.stack.len();

        let namespace_size = func.namespace_size;
        let locals_count = u16::try_from(namespace_size).expect("function namespace size exceeds u16");

        let mut frame_namespace_guard = DropGuard::new(frame_namespace, self);
        let (_, this) = frame_namespace_guard.as_parts_mut();

        // 1. Build the namespace in the reusable scratch buffer to avoid a
        //    per-call allocation. On error `DropGuard` drops the buffer, so the
        //    pool just restarts empty next call.
        let mut namespace = mem::take(&mut this.namespace_scratch);
        namespace.reserve(namespace_size);
        let mut namespace_guard = DropGuard::new(namespace, this);
        let (namespace, this) = namespace_guard.as_parts_mut();

        // 2. Bind arguments to parameters
        func.signature.bind(args, defaults, this, func.name, namespace)?;

        // 3. Install owned cells and captured free-var cells at their slots.
        this.install_closure_cells(func, cells, namespace);

        // 4. Commit the guards (no rollback) and push the frame. The operand
        // stack starts immediately above the locals region — comprehensions
        // emit their own push/pop bytecode, so no frame-level region is
        // reserved here. `append` empties the buffer (keeping its allocation)
        // so it can return to the pool.
        let (mut namespace, this) = namespace_guard.into_parts();
        this.stack.append(&mut namespace);
        this.namespace_scratch = namespace;
        let (frame_namespace, this) = frame_namespace_guard.into_parts();

        let exc_stack_base = this.exception_stack.len();
        this.push_frame(CallFrame::new_function(
            &func.code,
            stack_base,
            locals_count,
            exc_stack_base,
            func_id,
            call_offset,
            frame_namespace,
        ))?;

        Ok(CallResult::FramePushed)
    }

    /// Constructs an instance of a user-defined class — the `Foo(...)` path.
    ///
    /// Allocates the instance with an empty `__dict__`, then:
    /// - **No `__init__`:** rejects any arguments (like `object()`), returns the
    ///   instance directly.
    /// - **`__init__` is a plain sync function** (the normal case): pushes the
    ///   instance onto the operand stack as the pending result, runs
    ///   `__init__(self, *args)` as a real (suspendable) frame, and marks that
    ///   frame `is_initializer`. When the initializer frame returns, the
    ///   [`ReturnValue`](crate::bytecode::Opcode::ReturnValue) handler enforces the
    ///   `None` return and leaves the already-pushed instance as the result — so
    ///   `Foo(a)` evaluates to the new instance, not `__init__`'s return.
    /// - **Any other `__init__`** (builtin, class, `async def`, non-callable, ...):
    ///   runs it to completion synchronously via
    ///   [`evaluate_function`](Self::evaluate_function) and enforces CPython's
    ///   contract that it returns `None`. This path cannot suspend, and must NOT
    ///   go through the frame-marking path: a class-valued `__init__` recurses
    ///   into `instantiate_class`, which pushes its own pending instance —
    ///   blindly marking the resulting frame would corrupt the operand stack.
    ///
    /// Because a plain-function `__init__` runs as a normal frame, it may suspend
    /// on external/OS calls; the `is_initializer` flag is threaded through frame
    /// serialization so a suspended initializer resumes correctly.
    pub(crate) fn instantiate_class(&mut self, class_id: HeapId, args: ArgValues) -> Result<CallResult, RunError> {
        if let Some(new_func) = class_lookup(class_id, "__new__", self.heap, self.interns) {
            let new_func = new_func.clone_with_heap(self.heap);
            return self.instantiate_with_new(class_id, new_func, args);
        }
        let instance_id = self
            .heap
            .allocate(HeapData::Instance(Box::new(Instance::new(class_id, Dict::new()))));
        // The instance now owns a reference to its class object.
        self.heap.inc_ref(class_id);
        self.initialize_instance(class_id, instance_id, args)
    }

    /// `cls(*args)` for a class whose MRO defines `__new__`, as CPython's
    /// `type.__call__`: `__new__` (an implicit staticmethod, so passed `cls`)
    /// runs to completion, then `__init__` runs on its result only if that is
    /// an instance of `cls`; anything else is returned as is.
    fn instantiate_with_new(&mut self, class_id: HeapId, new_func: Value, args: ArgValues) -> RunResult<CallResult> {
        let this = self;
        defer_drop!(new_func, this);
        // Each synchronous re-entry restarts the dispatch countdown, so a tree of
        // short nested `__new__` calls would otherwise never reach a time check.
        if let Err(error) = this.heap.tracker.check_time() {
            args.drop_with(this);
            return Err(error.into());
        }
        let init_args = args.clone_with_heap(this.heap);
        let mut init_args = DropGuard::new(init_args, this);
        let this = init_args.ctx();
        this.heap.inc_ref(class_id);
        let instance = this.evaluate_function(new_func, args.prepend(Value::Ref(class_id)))?;
        let (init_args, this) = init_args.into_parts();
        let instance_class = match instance.py_type(this) {
            TypeRef::Class(actual) if TypeRef::Class(actual).is_subclass_of(TypeRef::Class(class_id), this.heap) => {
                actual
            }
            _ => {
                init_args.drop_with(this);
                return Ok(CallResult::Value(instance));
            }
        };
        // `initialize_instance` takes over the reference `__new__` returned.
        let instance_id = instance.into_ref_id().expect("class instances are heap values");
        this.initialize_instance(instance_class, instance_id, init_args)
    }

    /// Runs `__init__` on a freshly built instance of `class_id`, taking its
    /// reference: the instance is the result, or is released on error.
    fn initialize_instance(&mut self, class_id: HeapId, instance_id: HeapId, args: ArgValues) -> RunResult<CallResult> {
        // Look up `__init__` through the MRO (cloned out to release the borrow).
        let init = class_lookup(class_id, "__init__", self.heap, self.interns).map(|v| v.clone_with_heap(self));

        match init {
            None if matches!(args, ArgValues::Empty) => Ok(CallResult::Value(Value::Ref(instance_id))),
            // `object.__init__` ignores extra arguments when `__new__` is overridden.
            None if class_defines(class_id, "__new__", self) => {
                args.drop_with(self);
                Ok(CallResult::Value(Value::Ref(instance_id)))
            }
            None => {
                args.drop_with(self);
                let err = takes_no_arguments(class_id, self);
                Value::Ref(instance_id).drop_with(self);
                Err(err)
            }
            // A `@dataclass`-generated `__init__` binds its fields natively,
            // with no generated bytecode; holding `init_func` keeps its plan
            // alive while factories run.
            Some(init_func) if dataclasses::is_generated_init(&init_func, self) => {
                let this = self;
                defer_drop!(init_func, this);
                dataclasses::run_init(this, init_func, Value::Ref(instance_id), args).map(CallResult::Value)
            }
            Some(init_func) => {
                let this = self;
                defer_drop!(init_func, this);
                // CPython's `type.__call__` looks up `__init__` with descriptor
                // binding: only plain functions bind the new instance as `self`.
                // Bound methods already carry their own receiver, and builtins,
                // classes and other values are called with the constructor
                // arguments unchanged.
                let init_args = if this.is_function_value(init_func) {
                    this.heap.inc_ref(instance_id);
                    args.prepend(Value::Ref(instance_id))
                } else {
                    args
                };
                if this.is_plain_sync_function(init_func) {
                    // Push the instance as the pending result (transferring the
                    // allocation's reference), then run __init__ as a real
                    // (suspendable) frame.
                    this.push(Value::Ref(instance_id));
                    match this.call_function(init_func, init_args)? {
                        CallResult::FramePushed => {
                            // Mark the just-pushed frame so its return value is
                            // discarded (after the `None` check in the ReturnValue
                            // handler) and the pending instance becomes the result.
                            this.current_frame_mut().is_initializer = true;
                            Ok(CallResult::FramePushed)
                        }
                        other => {
                            // Defensive: `is_plain_sync_function` guarantees a frame push.
                            other.drop_with(this);
                            this.pop().drop_with(this);
                            Err(ExcType::type_error("__init__() must be a regular function"))
                        }
                    }
                } else {
                    // Exotic `__init__` (builtin, class, `async def`, non-callable,
                    // ...): run to completion synchronously — no pending instance is
                    // pushed — and enforce CPython's `None`-return contract.
                    match this.evaluate_function(init_func, init_args) {
                        Ok(Value::None) => Ok(CallResult::Value(Value::Ref(instance_id))),
                        Ok(result) => {
                            let type_name = result.py_type_name(this);
                            result.drop_with(this);
                            Value::Ref(instance_id).drop_with(this);
                            Err(ExcType::type_error_init_return(type_name))
                        }
                        Err(e) => {
                            Value::Ref(instance_id).drop_with(this);
                            Err(e)
                        }
                    }
                }
            }
        }
    }

    /// Whether `value` is a plain Python function object (`def`, closure, or
    /// function-with-defaults — sync or async): the kinds that act as descriptors
    /// in CPython and therefore bind an instance when looked up as a class member.
    fn is_function_value(&self, value: &Value) -> bool {
        match value {
            Value::DefFunction(_) => true,
            Value::Ref(id) => matches!(
                self.heap.get(*id),
                HeapData::Closure(_) | HeapData::FunctionDefaults(_) | HeapData::DataclassMethod(_)
            ),
            _ => false,
        }
    }

    /// Whether calling `value` would push a regular synchronous frame
    /// (`CallResult::FramePushed`): a plain `def`, closure, function-with-defaults,
    /// or a bound method wrapping one — but not an `async def`, whose call creates
    /// a coroutine instead. Used by [`instantiate_class`](Self::instantiate_class)
    /// to decide whether `__init__` can run as a suspendable initializer frame.
    fn is_plain_sync_function(&self, value: &Value) -> bool {
        match value {
            Value::DefFunction(func_id) => !self.interns.get_function(*func_id).is_async,
            Value::Ref(id) => match self.heap.get(*id) {
                HeapData::Closure(closure) => !self.interns.get_function(closure.func_id).is_async,
                HeapData::FunctionDefaults(fd) => !self.interns.get_function(fd.func_id).is_async,
                // Bound methods never wrap another bound method, so this
                // recursion is at most one level deep.
                HeapData::BoundMethod(bm) => self.is_plain_sync_function(&bm.func),
                _ => false,
            },
            _ => false,
        }
    }
}

/// Copies the omitted trailing defaults into their vacant parameter slots.
fn fill_default_slots(values: &mut [Value], defaults: &[Value], mut missing: u32, heap: &impl ContainsHeap) {
    while missing != 0 {
        let slot = missing.trailing_zeros() as usize;
        missing &= missing - 1;
        let default_index = defaults.len() - (values.len() - slot);
        values[slot] = defaults[default_index].clone_with_heap(heap);
    }
}

/// Asserts a callable popped off the stack by an exact-positional-call fast
/// path is the function that fast path was chosen for.
///
/// Shared by [`VM::call_exact_sync_function`] and [`VM::create_exact_coroutine`],
/// which both remove their callable from the stack after already having
/// dispatched on its `FunctionId` in [`VM::try_call_exact_def_function`].
fn debug_assert_exact_callable(callable: &Value, func_id: FunctionId) {
    debug_assert!(matches!(callable, Value::DefFunction(id) if *id == func_id));
}

/// Centralised dunder dispatch for `__enter__` / `__exit__` (and, when added,
/// any other dunder that maps to a [`PyTrait`] method).
///
/// Returns `Some(result)` when `name_id` names a recognised dunder — `args`
/// is taken out of the slot and consumed. Returns `None` when it isn't —
/// `args` is left untouched in the slot so the caller can hand it off to
/// the regular `py_call_attr` dispatch.
///
/// The `&mut Option<ArgValues>` shape is what keeps "all the recognition
/// and dispatch logic in one function" honest: `args` is non-`Copy` and
/// has a `Drop` impl that panics on stray `Ref` values, so it can only be
/// passed by value once we know we'll consume it.
///
/// Adding a new dunder is just a new arm in the inner `match`; type
/// implementations only need to override the corresponding `PyTrait`
/// method, never a `StaticStrings::Foo` arm in their `py_call_attr`.
fn dispatch_dunder(
    name_id: StringId,
    heap_id: HeapId,
    vm: &mut VM<'_>,
    args: &mut Option<ArgValues>,
) -> Option<Result<CallResult, RunError>> {
    let static_str = vm.interns.static_string(name_id)?;
    // User-defined instances are never intercepted: an explicit
    // `obj.__enter__()` / `obj.__exit__(a, b, c)` on an instance is an
    // ordinary method call in CPython — the instance `__dict__` can shadow
    // the class method and the arguments must reach the user function
    // verbatim (the trait hooks reduce them to an `Option<HeapId>`, which
    // is lossy). The `with` statement still uses the trait hooks via the
    // `BeforeWith`/`WithExit`/`WithExceptStart` opcodes, which perform the
    // CPython type-level (class-only) lookup.
    if matches!(vm.heap.get(heap_id), HeapData::Instance(_)) {
        return None;
    }
    Some(match static_str {
        StaticStrings::Enter => {
            let args = args.take().expect("dispatch_dunder called with empty args slot");
            args.check_zero_args("__enter__", vm.heap)
                .and_then(|()| PyValue::from_heap(heap_id, vm.heap).py_enter(vm))
        }
        StaticStrings::Exit => {
            let args = args.take().expect("dispatch_dunder called with empty args slot");
            dispatch_exit(heap_id, vm, args)
        }
        _ => return None,
    })
}

/// Direct `obj.__exit__(typ, val, tb)` invocation.
///
/// Validates that exactly three positional arguments are passed (CPython
/// raises `TypeError` for any other arity) and forwards `val` to
/// [`PyTrait::py_exit`] as `Option<HeapId>`:
///
/// - `val is None` → `None`, treated as the "normal exit" path.
/// - `val is a heap-allocated value` → `Some(heap_id)`. For built-in context
///   managers this is the exception instance, matching the `with`-statement
///   call shape.
/// - `val is a scalar (Int, Bool, …)` → `None`. The trait abstraction can
///   only carry `HeapId`s, so non-Ref values cannot be forwarded; in
///   practice no supported context manager inspects a non-exception `val`,
///   and CPython's behavior for such calls is implementation-defined per
///   the user-provided `__exit__`.
///
/// `typ` and `tb` are discarded: every implementation we have re-derives the
/// type from `val` and Monty has no traceback objects (see
/// `limitations/with.md`).
fn dispatch_exit(heap_id: HeapId, vm: &mut VM<'_>, args: ArgValues) -> Result<CallResult, RunError> {
    let positional = args.into_pos_only("__exit__", vm.heap)?;
    defer_drop!(positional, vm);
    let [typ, val, tb] = positional.as_slice() else {
        return Err(ExcType::type_error_arg_count("__exit__", 3, positional.len()));
    };
    let _ = (typ, tb);
    let exc = match val {
        Value::Ref(id) => Some(*id),
        _ => None,
    };
    PyValue::from_heap(heap_id, vm.heap).py_exit(vm, exc)
}

/// What a Python function value is called with: its id, captured cells, owned
/// clones of its defaults and its exec globals, as [`VM::function_parts`] unpacks them.
struct FunctionParts {
    func_id: FunctionId,
    cells: Box<[HeapId]>,
    /// OWNED clones, released by the caller.
    defaults: Vec<Value>,
    globals: Option<HeapId>,
}
