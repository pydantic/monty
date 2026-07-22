//! Function call helpers for the VM.
//!
//! This module contains the implementation of call-related opcodes and helper
//! functions for executing function calls. The main entry points are the `exec_*`
//! methods which are called from the VM's main dispatch loop.

use std::mem;

use monty_types::{OsFunctionCall, ResourceTracker};

use super::{CallFrame, VM, recursion::RunReentryGuard};
use crate::{
    args::{ArgValues, KwargsValues},
    asyncio::Coroutine,
    builtins::{Builtins, BuiltinsFunctions, BuiltinsFunctionsExt},
    bytecode::FrameExit,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError},
    function::Function,
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId},
    heap_data::CellValue,
    intern::{FunctionId, StaticStrings, StringId},
    types::{Dict, Instance, PyTrait, Type, bytes::call_bytes_method, instance::class_name, str::call_str_method},
    value::{EitherStr, Value},
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
    /// A new frame was pushed for a defined function call.
    /// The VM should reload its cached frame state.
    FramePushed,
    /// External function call requested - VM should pause and return to caller.
    /// The `EitherStr` is the name of the external function (interned or heap-owned).
    External(EitherStr, ArgValues),
    /// OS operation call requested - VM should yield `FrameExit::OsCall` to host.
    ///
    /// The host executes the OS operation and resumes the VM with the result.
    /// The [`OsFunctionCall`] is a tagged enum whose variants carry their own
    /// typed args, so no separate `ArgValues` is needed at this layer.
    OsCall(OsFunctionCall),
    /// Dataclass method call requested - VM should yield `FrameExit::MethodCall` to host.
    ///
    /// The method name (e.g. `"distance"`) and the args include the dataclass instance
    /// as the first argument (`self`). Unlike `External`, this uses an `EitherStr` instead
    /// of `StringId` because method names are only known at runtime when dataclass
    /// inputs are provided.
    MethodCall(EitherStr, ArgValues),
    /// The call returned a value that should be implicitly awaited.
    ///
    /// Used by `asyncio.run()` to execute a coroutine without an explicit `await`.
    /// The VM will push the value onto the stack and execute `exec_get_awaitable`.
    AwaitValue(Value),
    /// OS call whose result must be stored into a heap [`OpenFile`](crate::types::OpenFile)'s
    /// buffer rather than pushed onto the operand stack.
    ///
    /// Used by `read(N)` / `readline()` / `readlines()` / `seek()` on the first
    /// operation that needs the full file content. The host services the OS
    /// call (always `ReadText` or `ReadBytes` against the file referenced by
    /// `file_id`); on resume the VM stores the returned content into
    /// `OpenFile::buffer` and then consumes the file's `pending_read`
    /// [`ReadSpec`](crate::types::ReadSpec) to compute the slice that becomes
    /// the call's return value.
    ///
    /// The OS-call payload is a [`OsFunctionCall::ReadText`] /
    /// [`OsFunctionCall::ReadBytes`] (the only legal variants here) carrying
    /// the file's virtual path; the per-call slice spec lives on the
    /// `OpenFile` itself (in `pending_read`), so this variant only needs to
    /// carry the typed call plus the file id used to look up the buffer slot.
    OsCallStoreBuffer { call: OsFunctionCall, file_id: HeapId },
}

impl<C: ContainsHeap> DropWithContext<C> for CallResult {
    fn drop_with(self, heap: &mut C) {
        match self {
            Self::Value(value) | Self::AwaitValue(value) => value.drop_with(heap),
            Self::External(_, args) | Self::MethodCall(_, args) => {
                args.drop_with(heap);
            }
            Self::OsCall(call) => call.drop_with(heap),
            Self::FramePushed => {}
            Self::OsCallStoreBuffer { call, file_id } => {
                call.drop_with(heap);
                // Single pin (see `inc_ref_for_pending_oscall`): release one ref
                // if the call is discarded before dispatch routes it to a
                // `pending_file_effect`.
                heap.heap_mut().dec_ref(file_id);
            }
        }
    }
}

impl<T: ResourceTracker> VM<'_, T> {
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
        let args = self.pop_n_args(arg_count);
        let callable = self.pop();
        let this = self;
        defer_drop!(callable, this);
        this.call_function(callable, args)
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
    pub(super) fn exec_call_builtin_type(&mut self, type_id: u8, arg_count: usize) -> Result<Value, RunError> {
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
    /// Pops the callable, positional args, and keyword args from the stack,
    /// builds the appropriate `ArgValues`, and calls the function.
    pub(super) fn exec_call_function_kw(
        &mut self,
        pos_count: usize,
        kwname_ids: Vec<StringId>,
    ) -> Result<CallResult, RunError> {
        let kw_count = kwname_ids.len();

        // Pop keyword values (TOS is last kwarg value)
        let kw_values = self.pop_n(kw_count);

        // Pop positional arguments
        let pos_args = self.pop_n(pos_count);

        // Pop the callable
        let callable = self.pop();
        let this = self;
        defer_drop!(callable, this);

        // Build kwargs as Vec<(StringId, Value)>
        let kwargs_inline: Vec<(StringId, Value)> = kwname_ids.into_iter().zip(kw_values).collect();

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
    /// Pops the object, positional args, and keyword args from the stack,
    /// builds the appropriate `ArgValues`, and calls the attribute.
    /// Returns a `CallResult` which may indicate an OS or external call.
    pub(super) fn exec_call_attr_kw(
        &mut self,
        name_id: StringId,
        pos_count: usize,
        kwname_ids: Vec<StringId>,
    ) -> Result<CallResult, RunError> {
        let kw_count = kwname_ids.len();

        // Pop keyword values (TOS is last kwarg value)
        let kw_values = self.pop_n(kw_count);

        // Pop positional arguments
        let pos_args = self.pop_n(pos_count);

        // Pop the object
        let obj = self.pop();

        // Build kwargs as Vec<(StringId, Value)>
        let kwargs_inline: Vec<(StringId, Value)> = kwname_ids.into_iter().zip(kw_values).collect();

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

    /// Executes `CallFunctionExtended` opcode.
    ///
    /// Handles calls with `*args` and/or `**kwargs` unpacking.
    pub(super) fn exec_call_function_extended(&mut self, has_kwargs: bool) -> Result<CallResult, RunError> {
        // Pop kwargs dict if present
        let kwargs = if has_kwargs { Some(self.pop()) } else { None };

        // Pop args tuple
        let args_tuple = self.pop();

        // Pop callable
        let callable = self.pop();

        // Unpack and call
        self.call_function_extended(callable, args_tuple, kwargs)
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
    /// For heap-allocated objects (`Value::Ref`), dispatches to the type's
    /// attribute call implementation via `py_call_attr`, which may return
    /// `CallResult::OsCall`, `CallResult::External`, or
    /// `CallResult::MethodCall` for operations that require host involvement.
    ///
    /// For interned strings (`Value::InternString`), uses the unified `call_str_method`.
    /// For interned bytes (`Value::InternBytes`), uses the unified `call_bytes_method`.
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
    fn call_attr(&mut self, obj: Value, name_id: StringId, args: ArgValues) -> Result<CallResult, RunError> {
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

        match obj {
            Value::Ref(heap_id) => {
                defer_drop!(obj, this);
                this.heap.read(heap_id).py_call_attr(heap_id, this, &attr, args)
            }
            Value::InternString(string_id) => {
                // Call string method on interned string literal using the unified dispatcher
                let s = this.interns.get_str(string_id);
                call_str_method(s, name_id, args, this).map(CallResult::Value)
            }
            Value::InternBytes(bytes_id) => {
                // Call bytes method on interned bytes literal using the unified dispatcher
                let b = this.interns.get_bytes(bytes_id);
                call_bytes_method(b, name_id, args, this).map(CallResult::Value)
            }
            Value::Builtin(Builtins::Type(t)) => {
                // Handle classmethods on type objects like dict.fromkeys()
                t.call_class_method(name_id, args, this).map(Into::into)
            }
            _ => {
                // Non-heap values without method support
                let type_name = obj.py_type_name(this);
                args.drop_with(this);
                Err(ExcType::attribute_error(type_name, this.interns.get_str(name_id)))
            }
        }
    }

    /// Evaluates a function in a synchronous position that cannot suspend to the host.
    ///
    /// User-defined functions run until their frame returns. An unresolved name raises
    /// `NameError`; external, OS, method, and future suspensions raise a variant-specific
    /// `NotImplementedError` because this context cannot preserve and resume its state.
    ///
    /// The nested `self.run()` below recurses on the native Rust stack, so
    /// re-entry is bounded via [`enter_run_reentry`](Self::enter_run_reentry)
    /// at entry — before `call_function`, since a class-valued `__init__` can
    /// recurse back in without ever pushing a frame.
    pub(crate) fn evaluate_function(
        &mut self,
        ctx: &'static str,
        callable: &Value,
        args: ArgValues,
    ) -> Result<Value, RunError> {
        if let Err(e) = self.enter_run_reentry() {
            // Bailing before `call_function` takes ownership of `args`, so
            // reclaim its refcounts here.
            args.drop_with(self);
            return Err(e.into());
        }
        let mut guard = RunReentryGuard::new(self);
        let this = &mut *guard;

        let exit = match this.call_function(callable, args)? {
            CallResult::Value(v) => return Ok(v),
            CallResult::FramePushed => {
                // A new frame was pushed for a defined function call - we need to run it
                // to completion.
                let stack_depth = this.frames.len();
                // Mark the frame as an exit point from the `run()` loop
                this.current_frame_mut().should_return = true;
                match this.run()? {
                    FrameExit::Return(v) => return Ok(v),
                    exit => {
                        // Pop frames off the stack from this failed evaluation
                        // (including the one just pushed)
                        while this.frames.len() >= stack_depth {
                            this.pop_frame();
                        }
                        exit
                    }
                }
            }
            unsupported => return Err(this.unsupported_call_result(ctx, unsupported)),
        };

        Err(this.unsupported_frame_exit(ctx, exit))
    }

    /// Converts a direct call suspension into a specific synchronous-context error.
    #[cold]
    fn unsupported_call_result(&mut self, ctx: &'static str, result: CallResult) -> RunError {
        let error = match &result {
            CallResult::External(function_name, _) => ExcType::not_implemented(format!(
                "{ctx}: external function '{}' is not yet supported in this context",
                function_name.as_str(self.interns)
            )),
            CallResult::OsCall(function_call) => ExcType::not_implemented(format!(
                "{ctx}: OS function '{}' is not yet supported in this context",
                function_call.name()
            )),
            CallResult::OsCallStoreBuffer { call, .. } => ExcType::not_implemented(format!(
                "{ctx}: OS function '{}' is not yet supported in this context",
                call.name()
            )),
            CallResult::MethodCall(method_name, _) => ExcType::not_implemented(format!(
                "{ctx}: method call '{}' is not yet supported in this context",
                method_name.as_str(self.interns)
            )),
            CallResult::AwaitValue(_) => {
                ExcType::not_implemented(format!("{ctx}: awaiting a value is not yet supported in this context"))
            }
            CallResult::Value(_) | CallResult::FramePushed => unreachable!("completed calls are handled above"),
        };
        result.drop_with(self);
        error.into()
    }

    /// Converts a nested VM suspension into a specific synchronous-context error.
    #[cold]
    fn unsupported_frame_exit(&mut self, ctx: &'static str, exit: FrameExit) -> RunError {
        let error = match &exit {
            FrameExit::Return(_) => unreachable!("return exits are handled above"),
            FrameExit::ExternalCall { function_name, .. } => ExcType::not_implemented(format!(
                "{ctx}: external function '{}' is not yet supported in this context",
                function_name.as_str(self.interns)
            )),
            FrameExit::OsCall { function_call, .. } => ExcType::not_implemented(format!(
                "{ctx}: OS function '{}' is not yet supported in this context",
                function_call.name()
            )),
            FrameExit::MethodCall { method_name, .. } => ExcType::not_implemented(format!(
                "{ctx}: method call '{}' is not yet supported in this context",
                method_name.as_str(self.interns)
            )),
            FrameExit::ResolveFutures(_) => ExcType::not_implemented(format!(
                "{ctx}: resolving async futures is not yet supported in this context"
            )),
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
    /// - `Value::ExtFunction`: returns `External` for caller to execute
    /// - `Value::DefFunction`: pushes a new frame, returns `FramePushed`
    /// - `Value::Ref`: checks for closure/function on heap
    pub(crate) fn call_function(&mut self, callable: &Value, args: ArgValues) -> Result<CallResult, RunError> {
        match callable {
            Value::Builtin(builtin) => builtin.call(self, args),
            Value::ModuleFunction(mf) => mf.call(self, args),
            Value::ExtFunction(name_id) => {
                // External function - return to caller to execute
                Ok(CallResult::External(EitherStr::Interned(*name_id), args))
            }
            Value::DefFunction(func_id) => {
                // Defined function without defaults or captured variables
                self.call_def_function(*func_id, &[], &[], args)
            }
            Value::Ref(heap_id) => {
                // Could be a closure or function with defaults - check heap
                self.call_heap_callable(*heap_id, args)
            }
            _ => {
                // Coupling check: reaching here means dispatch rejected the value,
                // so `Value::is_callable` must agree it is not callable.
                debug_assert!(
                    !callable.is_callable(self.heap),
                    "Value::is_callable accepts a value call_function rejects — the two drifted"
                );
                args.drop_with(self);
                let ty = callable.py_type_name(self);
                Err(ExcType::type_error(format!("'{ty}' object is not callable")))
            }
        }
    }

    /// Handles calling a heap-allocated callable (closure, function with defaults,
    /// external function, class constructor, or bound method).
    fn call_heap_callable(&mut self, heap_id: HeapId, args: ArgValues) -> Result<CallResult, RunError> {
        // Calling a class constructs an instance; calling a bound method prepends
        // its captured `self`. Both are dispatched before the closure/defaults
        // path because they don't fit the `(func_id, cells, defaults)` shape.

        let (func_id, cells, defaults) = match self.heap.get(heap_id) {
            HeapData::Class(_) => return self.instantiate_class(heap_id, args),
            HeapData::BoundMethod(bm) => {
                let instance = bm.instance.clone_with_heap(self);
                let func = bm.func.clone_with_heap(self);
                let this = self;
                defer_drop!(func, this);
                return this.call_function(func, args.prepend(instance));
            }
            HeapData::Closure(closure) => {
                let cloned_cells = closure.cells.clone();
                let cloned_defaults: Vec<Value> = closure.defaults.iter().map(|v| v.clone_with_heap(self)).collect();
                (closure.func_id, cloned_cells, cloned_defaults)
            }
            HeapData::FunctionDefaults(fd) => {
                let cloned_defaults: Vec<Value> = fd.defaults.iter().map(|v| v.clone_with_heap(self)).collect();
                (fd.func_id, Vec::new(), cloned_defaults)
            }
            HeapData::ExtFunction(name) => {
                // Heap-allocated external function with a non-interned name
                let name = name.clone();
                return Ok(CallResult::External(EitherStr::Heap(name), args));
            }
            _ => {
                // Coupling check: dispatch rejected this Ref, so the heap-side
                // callability predicate must agree (see `HeapData::is_callable`).
                debug_assert!(
                    !self.heap.get(heap_id).is_callable(),
                    "HeapData::is_callable accepts a heap value call_heap_callable rejects — the two drifted"
                );
                args.drop_with(self);
                let type_name = self.heap.get(heap_id).py_type().name(self.heap, self.interns);
                return Err(ExcType::type_error_not_callable_object(&type_name));
            }
        };

        let this = self;
        defer_drop!(defaults, this);
        this.call_def_function(func_id, &cells, defaults, args)
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

        // Extract positional args from tuple
        let copied_args = this.extract_args_tuple(args_tuple);

        // Build ArgValues from positional args and optional kwargs
        let args = if let Some(kwargs_ref) = kwargs {
            this.build_args_with_kwargs(copied_args, kwargs_ref)?
        } else {
            Self::build_args_positional_only(copied_args)
        };

        // Call the function (args_tuple guard drops at scope exit)
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

        // Extract positional args from tuple
        let copied_args = this.extract_args_tuple_for_attr(args_tuple);

        // Build ArgValues from positional args and optional kwargs
        let args = if let Some(kwargs_ref) = kwargs {
            this.build_args_with_kwargs_for_attr(copied_args, kwargs_ref)?
        } else {
            Self::build_args_positional_only(copied_args)
        };

        // Call the method (args_tuple guard drops at scope exit)
        this.call_attr(obj, name_id, args)
    }

    /// Extracts arguments from a tuple for `CallFunctionExtended`.
    ///
    /// # Panics
    /// Panics if `args_tuple` is not a tuple. This indicates a compiler bug since
    /// the compiler always emits `ListToTuple` before `CallFunctionExtended`.
    fn extract_args_tuple(&mut self, args_tuple: &Value) -> Vec<Value> {
        let Value::Ref(id) = args_tuple else {
            unreachable!("CallFunctionExtended: args_tuple must be a Ref")
        };
        let HeapData::Tuple(tuple) = self.heap.get(*id) else {
            unreachable!("CallFunctionExtended: args_tuple must be a Tuple")
        };
        tuple.as_slice().iter().map(|v| v.clone_with_heap(self)).collect()
    }

    /// Builds `ArgValues` with kwargs for `CallFunctionExtended`.
    ///
    /// # Panics
    /// Panics if `kwargs_ref` is not a dict. This indicates a compiler bug since
    /// the compiler always emits `BuildDict` before `CallFunctionExtended` with kwargs.
    fn build_args_with_kwargs(&mut self, copied_args: Vec<Value>, kwargs_ref: Value) -> Result<ArgValues, RunError> {
        let this = self;
        defer_drop!(kwargs_ref, this);

        // Extract kwargs dict items
        let Value::Ref(id) = kwargs_ref else {
            unreachable!("CallFunctionExtended: kwargs must be a Ref")
        };
        let HeapData::Dict(dict) = this.heap.get(*id) else {
            unreachable!("CallFunctionExtended: kwargs must be a Dict")
        };
        let copied_kwargs: Vec<(Value, Value)> = dict
            .iter()
            .map(|(k, v)| (k.clone_with_heap(this), v.clone_with_heap(this)))
            .collect();

        let kwargs_values = if copied_kwargs.is_empty() {
            KwargsValues::Empty
        } else {
            let kwargs_dict = Dict::from_pairs(copied_kwargs, this)?;
            KwargsValues::Dict(kwargs_dict)
        };

        Ok(
            if copied_args.is_empty() && matches!(kwargs_values, KwargsValues::Empty) {
                ArgValues::Empty
            } else if copied_args.is_empty() {
                ArgValues::Kwargs(kwargs_values)
            } else {
                ArgValues::ArgsKargs {
                    args: copied_args,
                    kwargs: kwargs_values,
                }
            },
        )
    }

    /// Builds `ArgValues` from positional args only.
    fn build_args_positional_only(copied_args: Vec<Value>) -> ArgValues {
        match copied_args.len() {
            0 => ArgValues::Empty,
            1 => ArgValues::One(copied_args.into_iter().next().unwrap()),
            2 => {
                let mut iter = copied_args.into_iter();
                ArgValues::Two(iter.next().unwrap(), iter.next().unwrap())
            }
            _ => ArgValues::ArgsKargs {
                args: copied_args,
                kwargs: KwargsValues::Empty,
            },
        }
    }

    /// Extracts arguments from a tuple for `CallAttrExtended`.
    ///
    /// # Panics
    /// Panics if `args_tuple` is not a tuple. This indicates a compiler bug since
    /// the compiler always emits `ListToTuple` before `CallAttrExtended`.
    fn extract_args_tuple_for_attr(&mut self, args_tuple: &Value) -> Vec<Value> {
        let Value::Ref(id) = args_tuple else {
            unreachable!("CallAttrExtended: args_tuple must be a Ref")
        };
        let HeapData::Tuple(tuple) = self.heap.get(*id) else {
            unreachable!("CallAttrExtended: args_tuple must be a Tuple")
        };
        tuple.as_slice().iter().map(|v| v.clone_with_heap(self)).collect()
    }

    /// Builds `ArgValues` with kwargs for `CallAttrExtended`.
    ///
    /// # Panics
    /// Panics if `kwargs_ref` is not a dict. This indicates a compiler bug since
    /// the compiler always emits `BuildDict` before `CallAttrExtended` with kwargs.
    fn build_args_with_kwargs_for_attr(
        &mut self,
        copied_args: Vec<Value>,
        kwargs_ref: Value,
    ) -> Result<ArgValues, RunError> {
        let this = self;
        defer_drop!(kwargs_ref, this);

        // Extract kwargs dict items
        let Value::Ref(id) = kwargs_ref else {
            unreachable!("CallAttrExtended: kwargs must be a Ref")
        };
        let HeapData::Dict(dict) = this.heap.get(*id) else {
            unreachable!("CallAttrExtended: kwargs must be a Dict")
        };
        let copied_kwargs: Vec<(Value, Value)> = dict
            .iter()
            .map(|(k, v)| (k.clone_with_heap(this), v.clone_with_heap(this)))
            .collect();

        let kwargs_values = if copied_kwargs.is_empty() {
            KwargsValues::Empty
        } else {
            let kwargs_dict = Dict::from_pairs(copied_kwargs, this)?;
            KwargsValues::Dict(kwargs_dict)
        };

        Ok(
            if copied_args.is_empty() && matches!(kwargs_values, KwargsValues::Empty) {
                ArgValues::Empty
            } else if copied_args.is_empty() {
                ArgValues::Kwargs(kwargs_values)
            } else {
                ArgValues::ArgsKargs {
                    args: copied_args,
                    kwargs: kwargs_values,
                }
            },
        )
    }

    // ========================================================================
    // Frame Setup
    // ========================================================================

    /// Calls a defined function by pushing a new frame or creating a coroutine.
    ///
    /// For sync functions: sets up the function's namespace with bound arguments,
    /// cell variables, and free variables, then pushes a new frame.
    ///
    /// For async functions: binds arguments immediately but returns a Coroutine
    /// instead of pushing a frame. The coroutine stores the pre-bound namespace
    /// and will be executed when awaited.
    fn call_def_function(
        &mut self,
        func_id: FunctionId,
        cells: &[HeapId],
        defaults: &[Value],
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        let func = self.interns.get_function(func_id);

        if func.is_async {
            self.create_coroutine(func_id, cells, defaults, args)
        } else {
            self.call_sync_function(func_id, cells, defaults, args)
        }
    }

    /// Creates a Coroutine for an async function call.
    ///
    /// The coroutine is executed when awaited via Await.
    fn create_coroutine(
        &mut self,
        func_id: FunctionId,
        cells: &[HeapId],
        defaults: &[Value],
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        let func = self.interns.get_function(func_id);

        // 1. Create namespace for the coroutine with bound arguments and captured cells.
        let namespace = Vec::with_capacity(func.namespace_size);
        let mut namespace_guard = DropGuard::new(namespace, self);
        let (namespace, this) = namespace_guard.as_parts_mut();

        // 2. Bind arguments to parameters
        func.signature.bind(args, defaults, this, func.name, namespace)?;

        // 3. Install owned cells and captured free-var cells at their slots.
        this.install_closure_cells(func, cells, namespace)?;

        // 4. Create Coroutine on heap
        let (namespace, this) = namespace_guard.into_parts();
        let coroutine = Coroutine::new(func_id, namespace);
        let coroutine_id = this.heap.allocate(HeapData::Coroutine(coroutine))?;

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
    /// coroutine creation.
    fn install_closure_cells(
        &mut self,
        func: &Function,
        cells: &[HeapId],
        namespace: &mut Vec<Value>,
    ) -> Result<(), RunError> {
        namespace.resize_with(func.namespace_size, || Value::Undefined);

        for (i, &slot) in func.cell_var_slots.iter().enumerate() {
            let cell_value = match func.cell_param_indices[i] {
                Some(param_idx) => namespace[param_idx].clone_with_heap(self),
                None => Value::Undefined,
            };
            let cell_id = self.heap.allocate(HeapData::Cell(CellValue(cell_value)))?;
            namespace[slot.index()] = Value::Ref(cell_id);
        }

        for (i, &cell_id) in cells.iter().enumerate() {
            self.heap.inc_ref(cell_id);
            namespace[func.free_var_slots[i].index()] = Value::Ref(cell_id);
        }

        Ok(())
    }

    /// Calls a sync function by pushing a new frame.
    ///
    /// Sets up the function's namespace with bound arguments, cell variables,
    /// and free variables (captured from enclosing scope for closures).
    ///
    /// Locals are built in the reusable `namespace_scratch` buffer (under a
    /// [`DropGuard`] for cleanup on error) and moved onto the VM stack, where
    /// `stack_base` points to the start of the locals region.
    fn call_sync_function(
        &mut self,
        func_id: FunctionId,
        cells: &[HeapId],
        defaults: &[Value],
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        let call_offset = self.current_offset();
        let stack_base = self.stack.len();

        let func = self.interns.get_function(func_id);
        let namespace_size = func.namespace_size;
        let locals_count = u16::try_from(namespace_size).expect("function namespace size exceeds u16");

        // Track memory for this frame's locals. Symmetric with
        // `cleanup_frame_state`. Comprehension variables live on the operand
        // stack (pushed per-comp), not in any frame-level region, so they
        // don't enter this accounting.
        let size = namespace_size * mem::size_of::<Value>();
        self.heap.tracker_mut().on_grow(|| size)?;

        // 1. Build the namespace in the reusable scratch buffer to avoid a
        //    per-call allocation. On error `DropGuard` drops the buffer, so the
        //    pool just restarts empty next call.
        let mut namespace = mem::take(&mut self.namespace_scratch);
        namespace.reserve(namespace_size);
        let mut namespace_guard = DropGuard::new(namespace, self);
        let (namespace, this) = namespace_guard.as_parts_mut();

        // 2. Bind arguments to parameters
        {
            let bind_result = func.signature.bind(args, defaults, this, func.name, namespace);

            if let Err(e) = bind_result {
                this.heap.tracker_mut().on_free(|| size);
                return Err(e);
            }
        }

        // 3. Install owned cells and captured free-var cells at their slots.
        this.install_closure_cells(func, cells, namespace)?;

        let code = &func.code;

        // 6. Commit the guard (no rollback) and push the frame. The operand
        // stack starts immediately above the locals region — comprehensions
        // emit their own push/pop bytecode, so no frame-level region is
        // reserved here. `append` empties the buffer (keeping its allocation)
        // so it can return to the pool.
        let (mut namespace, this) = namespace_guard.into_parts();
        this.stack.append(&mut namespace);
        this.namespace_scratch = namespace;

        let exc_stack_base = this.exception_stack.len();
        this.push_frame(CallFrame::new_function(
            code,
            stack_base,
            locals_count,
            exc_stack_base,
            func_id,
            call_offset,
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
    fn instantiate_class(&mut self, class_id: HeapId, args: ArgValues) -> Result<CallResult, RunError> {
        // Allocate the instance. On allocation failure drop the args we own.
        let instance_id = match self
            .heap
            .allocate(HeapData::Instance(Instance::new(class_id, Dict::new())))
        {
            Ok(id) => id,
            Err(e) => {
                args.drop_with(self);
                return Err(e.into());
            }
        };
        // The instance now owns a reference to its class object.
        self.heap.inc_ref(class_id);

        // Look up `__init__` in the class namespace (cloned out to release the borrow).
        let init = match self.heap.get(class_id) {
            HeapData::Class(class) => class
                .namespace()
                .get_by_str("__init__", self.heap, self.interns)
                .map(|v| v.clone_with_heap(self)),
            _ => None,
        };

        match init {
            None => {
                if matches!(args, ArgValues::Empty) {
                    Ok(CallResult::Value(Value::Ref(instance_id)))
                } else {
                    args.drop_with(self);
                    let name = class_name(class_id, self.heap, self.interns);
                    Value::Ref(instance_id).drop_with(self);
                    Err(ExcType::type_error(format!("{name}() takes no arguments")))
                }
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
                    match this.evaluate_function("__init__", init_func, init_args) {
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
            Value::Ref(id) => matches!(self.heap.get(*id), HeapData::Closure(_) | HeapData::FunctionDefaults(_)),
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
fn dispatch_dunder<T: ResourceTracker>(
    name_id: StringId,
    heap_id: HeapId,
    vm: &mut VM<'_, T>,
    args: &mut Option<ArgValues>,
) -> Option<Result<CallResult, RunError>> {
    let static_str = StaticStrings::from_string_id(name_id)?;
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
                .and_then(|()| vm.heap.read(heap_id).py_enter(heap_id, vm))
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
fn dispatch_exit<T: ResourceTracker>(
    heap_id: HeapId,
    vm: &mut VM<'_, T>,
    args: ArgValues,
) -> Result<CallResult, RunError> {
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
    vm.heap.read(heap_id).py_exit(heap_id, vm, exc)
}
