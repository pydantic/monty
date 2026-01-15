//! Function call helpers for the VM.

use super::{CallFrame, VM};
use crate::{
    args::{ArgValues, KwargsValues},
    exception_private::{ExcType, RunError},
    heap::{HeapData, HeapId},
    intern::{ExtFunctionId, FunctionId, StringId},
    io::PrintWriter,
    resource::ResourceTracker,
    types::{Dict, PyTrait},
    value::{Attr, Value},
};

/// Result of calling a function.
///
/// Distinguishes between builtin function calls (which return a value immediately),
/// user function calls (which push a frame and continue execution), and external
/// function calls (which pause the VM).
pub(super) enum CallResult {
    /// Builtin function returned a value - push it onto the stack.
    Builtin(Value),
    /// User function call - frame was pushed, continue execution in VM loop.
    /// The return value will be pushed by ReturnValue opcode.
    UserFunction,
    /// External function call - VM should pause and return to caller.
    /// Contains (ext_function_id, args) where args preserves both positional and keyword arguments.
    ExternalCall(ExtFunctionId, ArgValues),
}

impl<T: ResourceTracker, P: PrintWriter> VM<'_, T, P> {
    /// Pops n arguments from the stack and wraps them in ArgValues.
    pub(super) fn pop_n_args(&mut self, n: usize) -> ArgValues {
        match n {
            0 => ArgValues::Empty,
            1 => ArgValues::One(self.pop()),
            2 => {
                let b = self.pop();
                let a = self.pop();
                ArgValues::Two(a, b)
            }
            _ => {
                let args = self.pop_n(n);
                ArgValues::ArgsKargs {
                    args,
                    kwargs: KwargsValues::Empty,
                }
            }
        }
    }

    /// Calls a method on an object.
    ///
    /// For heap-allocated objects (Value::Ref), dispatches to the type's
    /// `py_call_attr` implementation via `heap.call_attr()`.
    pub(super) fn call_method(&mut self, obj: Value, name_id: StringId, args: ArgValues) -> Result<Value, RunError> {
        let attr = Attr::Interned(name_id);

        if let Value::Ref(heap_id) = obj {
            // Call the method on the heap object
            let result = self.heap.call_attr(heap_id, &attr, args, self.interns);
            // Drop the object reference after the call
            obj.drop_with_heap(self.heap);
            result
        } else {
            // Non-heap values don't support method calls
            let type_name = obj.py_type(self.heap);
            args.drop_with_heap(self.heap);
            Err(ExcType::attribute_error(type_name, self.interns.get_str(name_id)))
        }
    }

    /// Calls a callable value with the given arguments.
    ///
    /// Returns `CallResult::Builtin(value)` for builtin functions,
    /// `CallResult::UserFunction` for user functions (frame was pushed), or
    /// `CallResult::ExternalCall` for external functions (VM should pause).
    pub(super) fn call_function(&mut self, callable: Value, args: ArgValues) -> Result<CallResult, RunError> {
        match callable {
            Value::Builtin(builtin) => {
                // Call the builtin function
                let result = builtin.call(self.heap, args, self.interns, self.print_writer)?;
                Ok(CallResult::Builtin(result))
            }
            Value::ExtFunction(ext_id) => {
                // External function - return to caller to execute
                // Preserve full ArgValues to keep both positional and keyword arguments
                Ok(CallResult::ExternalCall(ext_id, args))
            }
            Value::Function(func_id) => {
                // User function without defaults or captured variables (inline representation)
                self.call_user_function(func_id, &[], Vec::new(), args)?;
                Ok(CallResult::UserFunction)
            }
            Value::Ref(heap_id) => {
                // Could be a closure or function - check heap and extract info.
                // Two-phase approach to avoid borrow conflicts:
                // 1. Copy data without incrementing refcounts
                // 2. Increment refcounts after the borrow ends

                // Phase 1: Copy data (func_id, cells, defaults) without refcount changes
                let (func_id, cells, defaults) = match self.heap.get(heap_id) {
                    HeapData::Closure(fid, cells, defaults) => {
                        let cloned_cells = cells.clone();
                        // Use copy_for_extend to avoid refcount increment during borrow
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

                // Call the user function
                self.call_user_function(func_id, &cells, defaults, args)?;
                Ok(CallResult::UserFunction)
            }
            _ => {
                args.drop_with_heap(self.heap);
                Err(ExcType::type_error("object is not callable"))
            }
        }
    }

    /// Calls a function with unpacked args tuple and optional kwargs dict.
    ///
    /// This is used for `f(*args)` and `f(**kwargs)` style calls.
    pub(super) fn call_function_ex(
        &mut self,
        callable: Value,
        args_tuple: Value,
        kwargs: Option<Value>,
    ) -> Result<CallResult, RunError> {
        // Two-phase approach for extracting positional args to avoid borrow conflicts
        // Phase 1: Copy items without refcount changes
        let copied_args: Vec<Value> = if let Value::Ref(id) = &args_tuple {
            if let HeapData::Tuple(tuple) = self.heap.get(*id) {
                tuple.as_vec().iter().map(Value::copy_for_extend).collect()
            } else {
                callable.drop_with_heap(self.heap);
                args_tuple.drop_with_heap(self.heap);
                if let Some(k) = kwargs {
                    k.drop_with_heap(self.heap);
                }
                return Err(RunError::internal("CallFunctionEx: expected tuple for args"));
            }
        } else {
            callable.drop_with_heap(self.heap);
            args_tuple.drop_with_heap(self.heap);
            if let Some(k) = kwargs {
                k.drop_with_heap(self.heap);
            }
            return Err(RunError::internal("CallFunctionEx: expected tuple ref for args"));
        };

        // Phase 2: Increment refcounts for positional args
        for arg in &copied_args {
            if let Value::Ref(id) = arg {
                self.heap.inc_ref(*id);
            }
        }

        // Build ArgValues from positional args and optional kwargs
        let args = if let Some(kwargs_ref) = kwargs {
            // Extract kwargs dict items with two-phase approach
            // Phase 1: Copy items
            let copied_kwargs: Vec<(Value, Value)> = if let Value::Ref(id) = &kwargs_ref {
                if let HeapData::Dict(dict) = self.heap.get(*id) {
                    dict.iter()
                        .map(|(k, v)| (Value::copy_for_extend(k), Value::copy_for_extend(v)))
                        .collect()
                } else {
                    callable.drop_with_heap(self.heap);
                    args_tuple.drop_with_heap(self.heap);
                    kwargs_ref.drop_with_heap(self.heap);
                    for arg in copied_args {
                        arg.drop_with_heap(self.heap);
                    }
                    return Err(RunError::internal("CallFunctionEx: expected dict for kwargs"));
                }
            } else {
                callable.drop_with_heap(self.heap);
                args_tuple.drop_with_heap(self.heap);
                kwargs_ref.drop_with_heap(self.heap);
                for arg in copied_args {
                    arg.drop_with_heap(self.heap);
                }
                return Err(RunError::internal("CallFunctionEx: expected dict ref for kwargs"));
            };

            // Phase 2: Increment refcounts for kwargs
            for (k, v) in &copied_kwargs {
                if let Value::Ref(id) = k {
                    self.heap.inc_ref(*id);
                }
                if let Value::Ref(id) = v {
                    self.heap.inc_ref(*id);
                }
            }

            // Clean up the kwargs dict ref (we cloned the contents)
            kwargs_ref.drop_with_heap(self.heap);

            let kwargs_values = if copied_kwargs.is_empty() {
                KwargsValues::Empty
            } else {
                let kwargs_dict = Dict::from_pairs(copied_kwargs, self.heap, self.interns)?;
                KwargsValues::Dict(kwargs_dict)
            };

            if copied_args.is_empty() && matches!(kwargs_values, KwargsValues::Empty) {
                ArgValues::Empty
            } else if copied_args.is_empty() {
                ArgValues::Kwargs(kwargs_values)
            } else {
                ArgValues::ArgsKargs {
                    args: copied_args,
                    kwargs: kwargs_values,
                }
            }
        } else {
            // No kwargs
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
        };

        // Clean up the args tuple ref (we cloned the contents)
        args_tuple.drop_with_heap(self.heap);

        // Now call the function with the built ArgValues
        self.call_function(callable, args)
    }

    /// Calls a user-defined function by pushing a new frame.
    ///
    /// Sets up the function's namespace with bound arguments, cell variables,
    /// and free variables (captured from enclosing scope for closures).
    fn call_user_function(
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
        let namespace_size = func.namespace_size;
        let param_count = func.signature.total_slots();
        let cell_var_count = func.cell_var_count;
        let cell_param_indices = func.cell_param_indices.clone();
        let code = &func.code;

        // 1. Create new namespace for function
        let namespace_idx = self.namespaces.new_namespace(namespace_size, self.heap)?;

        // 2. Bind arguments to parameters
        {
            let namespace = self.namespaces.get_mut(namespace_idx).mut_vec();
            let bind_result = func
                .signature
                .bind(args, &defaults, self.heap, self.interns, func.name, namespace);

            if let Err(e) = bind_result {
                self.namespaces.drop_with_heap(namespace_idx, self.heap);
                // Clean up defaults before returning error
                for default in defaults {
                    default.drop_with_heap(self.heap);
                }
                return Err(e);
            }
        }

        // Clean up defaults - they were copied into the namespace by bind()
        // so we need to drop our ownership of the original refs
        for default in defaults {
            default.drop_with_heap(self.heap);
        }

        // Track created cell HeapIds for the frame
        let mut frame_cells: Vec<HeapId> = Vec::with_capacity(cell_var_count + cells.len());

        // 3. Create cells for variables captured by nested functions
        {
            let namespace = self.namespaces.get_mut(namespace_idx).mut_vec();
            for (i, maybe_param_idx) in cell_param_indices.iter().enumerate() {
                let cell_slot = param_count + i;
                let cell_value = if let Some(param_idx) = maybe_param_idx {
                    // Cell is for a parameter - copy its value
                    namespace[*param_idx].clone_with_heap(self.heap)
                } else {
                    Value::Undefined
                };
                let cell_id = self.heap.allocate(HeapData::Cell(cell_value))?;
                frame_cells.push(cell_id);
                // Extend namespace to fit cell if needed
                while namespace.len() <= cell_slot {
                    namespace.push(Value::Undefined);
                }
                namespace[cell_slot] = Value::Ref(cell_id);
            }

            // 4. Copy captured cells (free vars) into namespace
            let free_var_start = param_count + cell_var_count;
            for (i, &cell_id) in cells.iter().enumerate() {
                self.heap.inc_ref(cell_id);
                frame_cells.push(cell_id);
                let slot = free_var_start + i;
                // Extend namespace to fit free var if needed
                while namespace.len() <= slot {
                    namespace.push(Value::Undefined);
                }
                namespace[slot] = Value::Ref(cell_id);
            }

            // 5. Fill remaining slots with Undefined
            while namespace.len() < namespace_size {
                namespace.push(Value::Undefined);
            }
        }

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
