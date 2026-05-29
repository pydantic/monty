//! Function call helpers for the VM.
//!
//! This module contains the implementation of call-related opcodes and helper
//! functions for executing function calls. The main entry points are the `exec_*`
//! methods which are called from the VM's main dispatch loop.

use std::mem;

use super::{CallFrame, VM};
use crate::{
    args::{ArgValues, KwargsValues},
    asyncio::Coroutine,
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::FrameExit,
    defer_drop,
    exception_private::{ExcType, RunError},
    heap::{ContainsHeap, DropWithHeap, HeapData, HeapGuard, HeapId},
    heap_data::CellValue,
    intern::{FunctionId, StaticStrings, StringId},
    os::OsFunctionCall,
    resource::ResourceTracker,
    types::{Dict, PyTrait, Type, bytes::call_bytes_method, str::call_str_method},
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

impl DropWithHeap for CallResult {
    fn drop_with_heap<H: ContainsHeap>(self, heap: &mut H) {
        match self {
            Self::Value(value) | Self::AwaitValue(value) => value.drop_with_heap(heap),
            Self::External(_, args) | Self::MethodCall(_, args) => {
                args.drop_with_heap(heap);
            }
            Self::OsCall(call) => call.drop_with_heap(heap),
            Self::FramePushed => {}
            Self::OsCallStoreBuffer { call, file_id } => {
                call.drop_with_heap(heap);
                let heap = heap.heap_mut();
                heap.dec_ref(file_id);
                heap.dec_ref(file_id);
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
                let type_name = obj.py_type(this);
                args.drop_with_heap(this);
                Err(ExcType::attribute_error(type_name, this.interns.get_str(name_id)))
            }
        }
    }

    /// Evaluates a function in a position that doesn't yet support suspending.
    ///
    /// Calls the function and, if it's a user-defined function that pushes a frame,
    /// runs the VM until that frame returns.
    ///
    /// Returns an error for external/OS functions since those require the host to
    /// execute them and resume, which this synchronous context cannot support.
    pub(crate) fn evaluate_function(
        &mut self,
        ctx: &'static str,
        callable: &Value,
        args: ArgValues,
    ) -> Result<Value, RunError> {
        match self.call_function(callable, args)? {
            CallResult::Value(v) => return Ok(v),
            CallResult::FramePushed => {
                // A new frame was pushed for a defined function call - we need to run it
                // to completion.
                let stack_depth = self.frames.len();
                // Mark the frame as an exit point from the `run()` loop
                self.current_frame_mut().should_return = true;
                match self.run()? {
                    FrameExit::Return(v) => return Ok(v),
                    exit => {
                        exit.drop_with_heap(self);
                        // Pop frames off the stack from this failed evaluation
                        // (including the one just pushed)
                        while self.frames.len() >= stack_depth {
                            self.pop_frame();
                        }
                    }
                }
            }
            other => other.drop_with_heap(self),
        }

        Err(ExcType::not_implemented(format!(
            "{ctx}: external functions are not yet supported in this context"
        ))
        .into())
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
                args.drop_with_heap(self);
                let ty = callable.py_type(self);
                Err(ExcType::type_error(format!("'{ty}' object is not callable")))
            }
        }
    }

    /// Handles calling a heap-allocated callable (closure, function with defaults, or external function).
    fn call_heap_callable(&mut self, heap_id: HeapId, args: ArgValues) -> Result<CallResult, RunError> {
        let (func_id, cells, defaults) = match self.heap.get(heap_id) {
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
                args.drop_with_heap(self);
                return Err(ExcType::type_error("object is not callable"));
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
        let mut namespace_guard = HeapGuard::new(namespace, self);
        let (namespace, this) = namespace_guard.as_parts_mut();

        // 2. Bind arguments to parameters
        func.signature.bind(args, defaults, this, func.name, namespace)?;

        // 3. Create cells for variables captured by nested functions
        {
            let param_count = func.signature.total_slots();
            for (i, maybe_param_idx) in func.cell_param_indices.iter().enumerate() {
                let cell_slot = param_count + i;
                let cell_value = if let Some(param_idx) = maybe_param_idx {
                    namespace[*param_idx].clone_with_heap(this)
                } else {
                    Value::Undefined
                };
                let cell_id = this.heap.allocate(HeapData::Cell(CellValue(cell_value)))?;
                namespace.resize_with(cell_slot, || Value::Undefined);
                namespace.push(Value::Ref(cell_id));
            }

            // 4. Copy captured cells (free vars) into namespace
            let free_var_start = param_count + func.cell_var_count;
            for (i, &cell_id) in cells.iter().enumerate() {
                this.heap.inc_ref(cell_id);
                let slot = free_var_start + i;
                namespace.resize_with(slot, || Value::Undefined);
                namespace.push(Value::Ref(cell_id));
            }

            // 5. Fill remaining slots with Undefined
            namespace.resize_with(func.namespace_size, || Value::Undefined);
        }

        // 6. Create Coroutine on heap
        let (namespace, this) = namespace_guard.into_parts();
        let coroutine = Coroutine::new(func_id, namespace);
        let coroutine_id = this.heap.allocate(HeapData::Coroutine(coroutine))?;

        Ok(CallResult::Value(Value::Ref(coroutine_id)))
    }

    /// Calls a sync function by pushing a new frame.
    ///
    /// Sets up the function's namespace with bound arguments, cell variables,
    /// and free variables (captured from enclosing scope for closures).
    ///
    /// Locals are built directly on the VM stack using a [`StackGuard`] that
    /// automatically rolls back on error. The frame's `stack_base` points to
    /// the start of this locals region, and operands are pushed above it.
    ///
    /// The call position is captured from [`current_position`](Self::current_position),
    /// which returns `None` when no frames are on the stack (e.g. host-initiated
    /// calls via [`MontyRepl`](crate::MontyRepl)).
    fn call_sync_function(
        &mut self,
        func_id: FunctionId,
        cells: &[HeapId],
        defaults: &[Value],
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        let call_position = self.current_position();
        let stack_base = self.stack.len();

        let func = self.interns.get_function(func_id);
        let namespace_size = func.namespace_size;
        let locals_count = u16::try_from(namespace_size).expect("function namespace size exceeds u16");

        // Track memory for this frame's locals. Symmetric with
        // `cleanup_frame_state`. Comprehension variables live on the operand
        // stack (pushed per-comp), not in any frame-level region, so they
        // don't enter this accounting.
        let size = namespace_size * mem::size_of::<Value>();
        self.heap.tracker_mut().on_allocate(|| size)?;

        // 1. Create namespace for the frame in a temporary vec, will extend to stack later
        let namespace = Vec::with_capacity(func.namespace_size);
        let mut namespace_guard = HeapGuard::new(namespace, self);
        let (namespace, this) = namespace_guard.as_parts_mut();

        // 2. Bind arguments to parameters
        {
            let bind_result = func.signature.bind(args, defaults, this, func.name, namespace);

            if let Err(e) = bind_result {
                this.heap.tracker_mut().on_free(|| size);
                return Err(e);
            }
        }

        // 3. Create cells for variables captured by nested functions
        {
            let param_count = func.signature.total_slots();
            for (i, maybe_param_idx) in func.cell_param_indices.iter().enumerate() {
                let cell_slot = param_count + i;
                let cell_value = if let Some(param_idx) = maybe_param_idx {
                    namespace[*param_idx].clone_with_heap(this)
                } else {
                    Value::Undefined
                };
                let cell_id = this.heap.allocate(HeapData::Cell(CellValue(cell_value)))?;
                namespace.resize_with(cell_slot, || Value::Undefined);
                namespace.push(Value::Ref(cell_id));
            }

            // 4. Copy captured cells (free vars) into namespace
            let free_var_start = param_count + func.cell_var_count;
            for (i, &cell_id) in cells.iter().enumerate() {
                this.heap.inc_ref(cell_id);
                let slot = free_var_start + i;
                namespace.resize_with(slot, || Value::Undefined);
                namespace.push(Value::Ref(cell_id));
            }

            // 5. Fill remaining slots with Undefined
            namespace.resize_with(namespace_size, || Value::Undefined);
        }

        let code = &func.code;

        // 6. Commit the guard (no rollback) and push the frame. The operand
        // stack starts immediately above the locals region — any
        // comprehensions emit their own push/pop bytecode at entry/exit, so
        // no frame-level region is reserved here.
        let (namespace, this) = namespace_guard.into_parts();
        this.stack.extend(namespace);

        let exc_stack_base = this.exception_stack.len();
        this.push_frame(CallFrame::new_function(
            code,
            stack_base,
            locals_count,
            exc_stack_base,
            func_id,
            call_position,
        ))?;

        Ok(CallResult::FramePushed)
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
