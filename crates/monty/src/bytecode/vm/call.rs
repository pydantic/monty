//! Function call helpers for the VM.
//!
//! This module contains the implementation of call-related opcodes and helper
//! functions for executing function calls. The main entry points are the `exec_*`
//! methods which are called from the VM's main dispatch loop.

use super::{CallFrame, VM};
use crate::{
    args::{ArgValues, KwargsValues},
    builtins::{Builtins, BuiltinsFunctions},
    exception_private::{ExcType, RunError},
    heap::{Heap, HeapData, HeapId},
    intern::{ExtFunctionId, FunctionId, Interns, StaticStrings, StringId},
    io::PrintWriter,
    resource::ResourceTracker,
    types::{
        Dict, PyTrait, Type,
        bytes::{bytes_fromhex, call_bytes_method},
        dict::dict_fromkeys,
        list::do_list_sort,
        str::call_str_method,
    },
    value::{Attr, Value},
};

/// Result of executing a call opcode.
///
/// Used by the `exec_*` methods to communicate what action the VM's main loop
/// should take after the call completes.
pub(super) enum CallResult {
    /// Call completed successfully - push this value onto the stack.
    Push(Value),
    /// A new frame was pushed for a defined function call.
    /// The VM should reload its cached frame state.
    FramePushed,
    /// External function call requested - VM should pause and return to caller.
    External(ExtFunctionId, ArgValues),
}

impl<T: ResourceTracker, P: PrintWriter> VM<'_, T, P> {
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
        self.call_function(callable, args)
    }

    /// Executes `CallBuiltinFunction` opcode.
    ///
    /// Calls a builtin function directly without stack manipulation for the callable.
    /// This is an optimization that avoids constant pool lookup and stack manipulation.
    pub(super) fn exec_call_builtin_function(&mut self, builtin_id: u8, arg_count: usize) -> Result<Value, RunError> {
        // Convert u8 to BuiltinsFunctions via FromRepr
        if let Some(builtin) = BuiltinsFunctions::from_repr(builtin_id) {
            let args = self.pop_n_args(arg_count);
            builtin.call(self.heap, args, self.interns, self.print_writer)
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
            t.call(self.heap, args, self.interns)
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

        self.call_function(callable, args)
    }

    /// Executes `CallMethod` opcode.
    ///
    /// Pops the object and arguments from the stack, calls the method,
    /// and returns the result value.
    pub(super) fn exec_call_method(&mut self, name_id: StringId, arg_count: usize) -> Result<Value, RunError> {
        let args = self.pop_n_args(arg_count);
        let obj = self.pop();
        self.call_method(obj, name_id, args)
    }

    /// Executes `CallMethodKw` opcode.
    ///
    /// Pops the object, positional args, and keyword args from the stack,
    /// builds the appropriate `ArgValues`, and calls the method.
    pub(super) fn exec_call_method_kw(
        &mut self,
        name_id: StringId,
        pos_count: usize,
        kwname_ids: Vec<StringId>,
    ) -> Result<Value, RunError> {
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

        self.call_method(obj, name_id, args)
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

    /// Calls a method on an object.
    ///
    /// For heap-allocated objects (`Value::Ref`), dispatches to the type's
    /// `py_call_attr` implementation via `heap.call_attr()`.
    /// For interned strings (`Value::InternString`), uses the unified `call_str_method`.
    /// For interned bytes (`Value::InternBytes`), uses the unified `call_bytes_method`.
    ///
    /// Special handling: `list.sort(key=...)` is intercepted here to allow calling
    /// builtin key functions with VM access.
    fn call_method(&mut self, obj: Value, name_id: StringId, args: ArgValues) -> Result<Value, RunError> {
        let attr = Attr::Interned(name_id);

        match obj {
            Value::Ref(heap_id) => {
                // Check for list.sort - needs special handling for key functions
                if name_id == StaticStrings::Sort && matches!(self.heap.get(heap_id), HeapData::List(_)) {
                    let result = do_list_sort(heap_id, args, self.heap, self.interns, self.print_writer);
                    obj.drop_with_heap(self.heap);
                    return result.map(|()| Value::None);
                }

                // Call the method on the heap object
                let result = self.heap.call_attr(heap_id, &attr, args, self.interns);
                // Drop the object reference after the call
                obj.drop_with_heap(self.heap);
                result
            }
            Value::InternString(string_id) => {
                // Call string method on interned string literal using the unified dispatcher
                let s = self.interns.get_str(string_id);
                call_str_method(s, name_id, args, self.heap, self.interns)
            }
            Value::InternBytes(bytes_id) => {
                // Call bytes method on interned bytes literal using the unified dispatcher
                let b = self.interns.get_bytes(bytes_id);
                call_bytes_method(b, name_id, args, self.heap, self.interns)
            }
            Value::Builtin(Builtins::Type(t)) => {
                // Handle classmethods on type objects like dict.fromkeys()
                call_type_method(t, name_id, args, self.heap, self.interns)
            }
            _ => {
                // Non-heap values without method support
                let type_name = obj.py_type(self.heap);
                args.drop_with_heap(self.heap);
                Err(ExcType::attribute_error(type_name, self.interns.get_str(name_id)))
            }
        }
    }

    /// Calls a callable value with the given arguments.
    ///
    /// Dispatches based on the callable type:
    /// - `Value::Builtin`: calls builtin directly, returns `Push`
    /// - `Value::ExtFunction`: returns `External` for caller to execute
    /// - `Value::DefFunction`: pushes a new frame, returns `FramePushed`
    /// - `Value::Ref`: checks for closure/function on heap
    fn call_function(&mut self, callable: Value, args: ArgValues) -> Result<CallResult, RunError> {
        match callable {
            Value::Builtin(builtin) => {
                let result = builtin.call(self.heap, args, self.interns, self.print_writer)?;
                Ok(CallResult::Push(result))
            }
            Value::ExtFunction(ext_id) => {
                // External function - return to caller to execute
                Ok(CallResult::External(ext_id, args))
            }
            Value::DefFunction(func_id) => {
                // Defined function without defaults or captured variables
                self.call_def_function(func_id, &[], Vec::new(), args)?;
                Ok(CallResult::FramePushed)
            }
            Value::Ref(heap_id) => {
                // Could be a closure or function with defaults - check heap
                self.call_heap_callable(heap_id, callable, args)
            }
            _ => {
                args.drop_with_heap(self.heap);
                Err(ExcType::type_error("object is not callable"))
            }
        }
    }

    /// Handles calling a heap-allocated callable (closure or function with defaults).
    ///
    /// Uses a two-phase approach to avoid borrow conflicts:
    /// 1. Copy data without incrementing refcounts
    /// 2. Increment refcounts after the borrow ends
    fn call_heap_callable(
        &mut self,
        heap_id: HeapId,
        callable: Value,
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        // Phase 1: Copy data (func_id, cells, defaults) without refcount changes
        let (func_id, cells, defaults) = match self.heap.get(heap_id) {
            HeapData::Closure(fid, cells, defaults) => {
                let cloned_cells = cells.clone();
                let cloned_defaults: Vec<Value> = defaults.iter().map(Value::copy_for_extend).collect();
                (*fid, cloned_cells, cloned_defaults)
            }
            HeapData::FunctionDefaults(fid, defaults) => {
                let cloned_defaults: Vec<Value> = defaults.iter().map(Value::copy_for_extend).collect();
                (*fid, Vec::new(), cloned_defaults)
            }
            _ => {
                callable.drop_with_heap(self.heap);
                args.drop_with_heap(self.heap);
                return Err(ExcType::type_error("object is not callable"));
            }
        };

        // Phase 2: Increment refcounts now that the heap borrow has ended
        for &cell_id in &cells {
            self.heap.inc_ref(cell_id);
        }
        for default in &defaults {
            if let Value::Ref(id) = default {
                self.heap.inc_ref(*id);
            }
        }

        // Drop the callable ref (cloned data has its own refcounts)
        callable.drop_with_heap(self.heap);

        // Call the defined function
        self.call_def_function(func_id, &cells, defaults, args)?;
        Ok(CallResult::FramePushed)
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
        // Extract positional args from tuple
        let copied_args = self.extract_args_tuple(&callable, &args_tuple, kwargs.as_ref())?;

        // Increment refcounts for positional args
        for arg in &copied_args {
            if let Value::Ref(id) = arg {
                self.heap.inc_ref(*id);
            }
        }

        // Build ArgValues from positional args and optional kwargs
        let args = if let Some(kwargs_ref) = kwargs {
            self.build_args_with_kwargs(copied_args, kwargs_ref, &callable, &args_tuple)?
        } else {
            Self::build_args_positional_only(copied_args)
        };

        // Clean up the args tuple ref (we cloned the contents)
        args_tuple.drop_with_heap(self.heap);

        // Call the function
        self.call_function(callable, args)
    }

    /// Extracts arguments from a tuple for `CallFunctionExtended`.
    fn extract_args_tuple(
        &mut self,
        callable: &Value,
        args_tuple: &Value,
        kwargs: Option<&Value>,
    ) -> Result<Vec<Value>, RunError> {
        if let Value::Ref(id) = args_tuple
            && let HeapData::Tuple(tuple) = self.heap.get(*id)
        {
            return Ok(tuple.as_vec().iter().map(Value::copy_for_extend).collect());
        }
        // Error case - clean up and return error
        callable.clone_immediate().drop_with_heap(self.heap);
        args_tuple.clone_immediate().drop_with_heap(self.heap);
        if let Some(k) = kwargs {
            k.clone_immediate().drop_with_heap(self.heap);
        }
        Err(RunError::internal("CallFunctionEx: expected tuple for args"))
    }

    /// Builds `ArgValues` with kwargs for `CallFunctionExtended`.
    fn build_args_with_kwargs(
        &mut self,
        copied_args: Vec<Value>,
        kwargs_ref: Value,
        callable: &Value,
        args_tuple: &Value,
    ) -> Result<ArgValues, RunError> {
        // Extract kwargs dict items
        let copied_kwargs: Vec<(Value, Value)> = if let Value::Ref(id) = &kwargs_ref {
            if let HeapData::Dict(dict) = self.heap.get(*id) {
                dict.iter()
                    .map(|(k, v)| (Value::copy_for_extend(k), Value::copy_for_extend(v)))
                    .collect()
            } else {
                self.cleanup_call_ex_error(callable, args_tuple, &kwargs_ref, copied_args);
                return Err(RunError::internal("CallFunctionEx: expected dict for kwargs"));
            }
        } else {
            self.cleanup_call_ex_error(callable, args_tuple, &kwargs_ref, copied_args);
            return Err(RunError::internal("CallFunctionEx: expected dict ref for kwargs"));
        };

        // Increment refcounts for kwargs
        for (k, v) in &copied_kwargs {
            if let Value::Ref(id) = k {
                self.heap.inc_ref(*id);
            }
            if let Value::Ref(id) = v {
                self.heap.inc_ref(*id);
            }
        }

        // Clean up the kwargs dict ref
        kwargs_ref.drop_with_heap(self.heap);

        let kwargs_values = if copied_kwargs.is_empty() {
            KwargsValues::Empty
        } else {
            let kwargs_dict = Dict::from_pairs(copied_kwargs, self.heap, self.interns)?;
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

    /// Cleans up values when `CallFunctionExtended` encounters an error.
    fn cleanup_call_ex_error(
        &mut self,
        callable: &Value,
        args_tuple: &Value,
        kwargs_ref: &Value,
        copied_args: Vec<Value>,
    ) {
        callable.clone_immediate().drop_with_heap(self.heap);
        args_tuple.clone_immediate().drop_with_heap(self.heap);
        kwargs_ref.clone_immediate().drop_with_heap(self.heap);
        for arg in copied_args {
            arg.drop_with_heap(self.heap);
        }
    }

    // ========================================================================
    // Frame Setup
    // ========================================================================

    /// Calls a defined function by pushing a new frame.
    ///
    /// Sets up the function's namespace with bound arguments, cell variables,
    /// and free variables (captured from enclosing scope for closures).
    fn call_def_function(
        &mut self,
        func_id: FunctionId,
        cells: &[HeapId],
        defaults: Vec<Value>,
        args: ArgValues,
    ) -> Result<(), RunError> {
        // Get call position BEFORE borrowing namespaces mutably
        let call_position = self.current_position();

        // Get function info (interns is a shared reference so no conflict)
        let func = self.interns.get_function(func_id);

        // 1. Create new namespace for function
        let namespace_idx = self.namespaces.new_namespace(func.namespace_size, self.heap)?;

        let namespace = self.namespaces.get_mut(namespace_idx).mut_vec();
        // 2. Bind arguments to parameters
        {
            let bind_result = func
                .signature
                .bind(args, &defaults, self.heap, self.interns, func.name, namespace);

            if let Err(e) = bind_result {
                self.namespaces.drop_with_heap(namespace_idx, self.heap);
                for default in defaults {
                    default.drop_with_heap(self.heap);
                }
                return Err(e);
            }
        }

        // Clean up defaults - they were copied into the namespace by bind()
        for default in defaults {
            default.drop_with_heap(self.heap);
        }

        // Track created cell HeapIds for the frame
        let mut frame_cells: Vec<HeapId> = Vec::with_capacity(func.cell_var_count + cells.len());

        // 3. Create cells for variables captured by nested functions
        {
            let param_count = func.signature.total_slots();
            for (i, maybe_param_idx) in func.cell_param_indices.iter().enumerate() {
                let cell_slot = param_count + i;
                let cell_value = if let Some(param_idx) = maybe_param_idx {
                    namespace[*param_idx].clone_with_heap(self.heap)
                } else {
                    Value::Undefined
                };
                let cell_id = self.heap.allocate(HeapData::Cell(cell_value))?;
                frame_cells.push(cell_id);
                namespace.resize_with(cell_slot, || Value::Undefined);
                namespace.push(Value::Ref(cell_id));
            }

            // 4. Copy captured cells (free vars) into namespace
            let free_var_start = param_count + func.cell_var_count;
            for (i, &cell_id) in cells.iter().enumerate() {
                self.heap.inc_ref(cell_id);
                frame_cells.push(cell_id);
                let slot = free_var_start + i;
                namespace.resize_with(slot, || Value::Undefined);
                namespace.push(Value::Ref(cell_id));
            }

            // 5. Fill remaining slots with Undefined
            namespace.resize_with(func.namespace_size, || Value::Undefined);
        }

        let code = &func.code;
        // 6. Push new frame
        self.frames.push(CallFrame::new_function(
            code,
            self.stack.len(),
            namespace_idx,
            func_id,
            frame_cells,
            call_position,
        ));

        Ok(())
    }
}

/// Dispatches a classmethod call on a type object.
///
/// Handles classmethods like `dict.fromkeys()` and `bytes.fromhex()` that are
/// called on the type itself rather than on an instance.
fn call_type_method(
    t: Type,
    method_id: StringId,
    args: ArgValues,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> Result<Value, RunError> {
    match (t, method_id) {
        (Type::Dict, m) if m == StaticStrings::Fromkeys => return dict_fromkeys(args, heap, interns),
        (Type::Bytes, m) if m == StaticStrings::Fromhex => return bytes_fromhex(args, heap, interns),
        _ => {}
    }
    // Other types or unknown methods - report actual type name, not 'type'
    args.drop_with_heap(heap);
    Err(ExcType::attribute_error(t, interns.get_str(method_id)))
}
