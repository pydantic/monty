//! Function call helpers for the VM.
//!
//! This module contains the implementation of call-related opcodes and helper
//! functions for executing function calls. The main entry points are the `exec_*`
//! methods which are called from the VM's main dispatch loop.

use std::str::FromStr;

use super::{CallFrame, PendingNewCall, VM};
use crate::{
    args::{ArgValues, KwargsValues},
    asyncio::Coroutine,
    builtins::{Builtins, BuiltinsFunctions},
    exception_private::{ExcType, RunError, RunResult},
    heap::{DropWithHeap, Heap, HeapData, HeapGuard, HeapId},
    intern::{ExtFunctionId, FunctionId, Interns, StaticStrings, StringId},
    io::PrintWriter,
    os::OsFunction,
    resource::ResourceTracker,
    types::{
        AttrCallResult, Dict, Instance, List, PyTrait, Type, UserProperty,
        bytes::{bytes_fromhex, call_bytes_method},
        class::PropertyAccessorKind,
        dict::dict_fromkeys,
        list::do_list_sort,
        make_generic_alias,
        str::call_str_method,
    },
    value::{EitherStr, Value},
};

/// Result of executing a call opcode.
///
/// Used by the `exec_*` methods to communicate what action the VM's main loop
/// should take after the call completes.
#[derive(Debug)]
pub(super) enum CallResult {
    /// Call completed successfully - push this value onto the stack.
    Push(Value),
    /// A new frame was pushed for a defined function call.
    /// The VM should reload its cached frame state.
    FramePushed,
    /// External function call requested - VM should pause and return to caller.
    External(ExtFunctionId, ArgValues),
    /// OS operation call requested - VM should yield `FrameExit::OsCall` to host.
    ///
    /// The host executes the OS operation and resumes the VM with the result.
    OsCall(OsFunction, ArgValues),
}

impl From<AttrCallResult> for CallResult {
    fn from(result: AttrCallResult) -> Self {
        match result {
            AttrCallResult::Value(v) => Self::Push(v),
            AttrCallResult::OsCall(func, args) => Self::OsCall(func, args),
            AttrCallResult::ExternalCall(ext_id, args) => Self::External(ext_id, args),
            AttrCallResult::PropertyCall(_, _) => {
                // PropertyCall should be handled by the VM's load_attr, not generic conversion.
                // This variant is only used by load_attr to defer property execution.
                // If we reach here, it indicates a bug in the VM's attribute loading logic.
                unreachable!("PropertyCall must be handled by load_attr, not generic conversion")
            }
            AttrCallResult::DescriptorGet(_) => {
                // DescriptorGet should be handled by the VM's load_attr, not generic conversion.
                // This variant is only used by load_attr to defer descriptor protocol execution.
                // If we reach here, it indicates a bug in the VM's attribute loading logic.
                unreachable!("DescriptorGet must be handled by load_attr, not generic conversion")
            }
        }
    }
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
    ///
    /// Intercepts certain builtins to dispatch dunders on instances:
    /// - `repr(x)` -> `x.__repr__()`
    /// - `hash(x)` -> `x.__hash__()`
    /// - `len(x)` -> `x.__len__()`
    /// - `abs(x)` -> `x.__abs__()`
    /// - `next(x)` -> `x.__next__()`
    pub(super) fn exec_call_builtin_function(
        &mut self,
        builtin_id: u8,
        arg_count: usize,
    ) -> Result<CallResult, RunError> {
        if let Some(builtin) = BuiltinsFunctions::from_repr(builtin_id) {
            // super() needs VM context (frame stack) - handle it here instead of in builtins
            if matches!(builtin, BuiltinsFunctions::Super) {
                let args = self.pop_n_args(arg_count);
                let result = self.call_super(args)?;
                return Ok(CallResult::Push(result));
            }

            // getattr/setattr/hasattr need dynamic string -> StringId conversion via mutable interns
            if matches!(
                builtin,
                BuiltinsFunctions::Getattr | BuiltinsFunctions::Setattr | BuiltinsFunctions::Hasattr
            ) {
                let args = self.pop_n_args(arg_count);
                let result = match builtin {
                    BuiltinsFunctions::Getattr => self.builtin_getattr(args)?,
                    BuiltinsFunctions::Setattr => self.builtin_setattr(args)?,
                    BuiltinsFunctions::Hasattr => self.builtin_hasattr(args)?,
                    _ => unreachable!(),
                };
                return Ok(CallResult::Push(result));
            }

            // Check for instance dunder dispatch on single-arg builtins
            if arg_count == 1 {
                let arg = self.peek();
                if let Value::Ref(arg_id) = arg
                    && matches!(self.heap.get(*arg_id), HeapData::Instance(_))
                {
                    let arg_id = *arg_id;
                    let dunder = match builtin {
                        BuiltinsFunctions::Repr => Some(StaticStrings::DunderRepr),
                        BuiltinsFunctions::Hash => Some(StaticStrings::DunderHash),
                        BuiltinsFunctions::Len => Some(StaticStrings::DunderLen),
                        BuiltinsFunctions::Abs => Some(StaticStrings::DunderAbs),
                        BuiltinsFunctions::Next => Some(StaticStrings::DunderNext),
                        _ => None,
                    };

                    if let Some(dunder_name) = dunder {
                        let dunder_id = dunder_name.into();
                        if let Some(method) = self.lookup_type_dunder(arg_id, dunder_id) {
                            let arg_val = self.pop();
                            let result = self.call_dunder(arg_id, method, ArgValues::Empty)?;
                            arg_val.drop_with_heap(self.heap);
                            return Ok(result);
                        }
                        // For hash(): if no __hash__ but has __eq__, raise TypeError
                        if matches!(builtin, BuiltinsFunctions::Hash) {
                            let eq_id = StaticStrings::DunderEq.into();
                            if let Some(eq_method) = self.lookup_type_dunder(arg_id, eq_id) {
                                // __eq__ defined without __hash__ - unhashable
                                eq_method.drop_with_heap(self.heap);
                                let arg_val = self.pop();
                                // Get class name
                                let class_name = match self.heap.get(arg_id) {
                                    HeapData::Instance(inst) => match self.heap.get(inst.class_id()) {
                                        HeapData::ClassObject(cls) => cls.name(self.interns).to_string(),
                                        _ => "instance".to_string(),
                                    },
                                    _ => "instance".to_string(),
                                };
                                arg_val.drop_with_heap(self.heap);
                                return Err(ExcType::type_error(format!("unhashable type: '{class_name}'")));
                            }
                        }
                    }
                }
            }

            let args = self.pop_n_args(arg_count);
            let result = builtin.call(self.heap, args, self.interns, self.print_writer)?;
            Ok(CallResult::Push(result))
        } else {
            Err(RunError::internal("CallBuiltinFunction: invalid builtin_id"))
        }
    }

    /// Executes `CallBuiltinType` opcode.
    ///
    /// Calls a builtin type constructor directly without stack manipulation for the callable.
    /// This is an optimization for type constructors like `list()`, `int()`, `str()`.
    ///
    /// For instances, intercepts to call dunder methods:
    /// - `str(x)` -> `x.__str__()` or `x.__repr__()`
    /// - `repr(x)` -> `x.__repr__()`
    /// - `int(x)` -> `x.__int__()`
    /// - `float(x)` -> `x.__float__()`
    /// - `bool(x)` -> `x.__bool__()` or `x.__len__()`
    /// - `hash(x)` -> `x.__hash__()`
    /// - `len(x)` -> `x.__len__()`
    pub(super) fn exec_call_builtin_type(&mut self, type_id: u8, arg_count: usize) -> Result<CallResult, RunError> {
        if let Some(t) = Type::callable_from_u8(type_id) {
            // Check if the single argument is an instance that has a relevant dunder
            if arg_count == 1 {
                // Peek at the arg (TOS) without popping
                let arg = self.peek();
                if let Value::Ref(arg_id) = arg
                    && matches!(self.heap.get(*arg_id), HeapData::Instance(_))
                {
                    let arg_id = *arg_id;
                    // Check for type-specific dunders
                    let dunder = match t {
                        Type::Str => Some((StaticStrings::DunderStr, Some(StaticStrings::DunderRepr))),
                        Type::Int => Some((StaticStrings::DunderInt, None)),
                        Type::Float => Some((StaticStrings::DunderFloat, None)),
                        Type::Bool => Some((StaticStrings::DunderBool, Some(StaticStrings::DunderLen))),
                        _ => None,
                    };

                    if let Some((primary_dunder, fallback_dunder)) = dunder {
                        let primary_id = primary_dunder.into();
                        if let Some(method) = self.lookup_type_dunder(arg_id, primary_id) {
                            // Pop the arg and call the dunder
                            let arg_val = self.pop();
                            let result = self.call_dunder(arg_id, method, ArgValues::Empty)?;
                            arg_val.drop_with_heap(self.heap);
                            return Ok(result);
                        }
                        // Try fallback dunder if primary not found
                        if let Some(fallback) = fallback_dunder {
                            let fallback_id = fallback.into();
                            if let Some(method) = self.lookup_type_dunder(arg_id, fallback_id) {
                                let arg_val = self.pop();
                                let result = self.call_dunder(arg_id, method, ArgValues::Empty)?;
                                arg_val.drop_with_heap(self.heap);
                                return Ok(result);
                            }
                        }
                    }
                }
            }

            let args = self.pop_n_args(arg_count);
            let result = t.call(self.heap, args, self.interns)?;
            Ok(CallResult::Push(result))
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
    /// `py_call_attr_raw` implementation via `heap.call_attr_raw()`, which may return
    /// `AttrCallResult::OsCall` or `AttrCallResult::ExternalCall` for operations that
    /// require host involvement.
    ///
    /// For interned strings (`Value::InternString`), uses the unified `call_str_method`.
    /// For interned bytes (`Value::InternBytes`), uses the unified `call_bytes_method`.
    ///
    /// Special handling: `list.sort(key=...)` is intercepted here to allow calling
    /// builtin key functions with VM access.
    pub(super) fn call_attr(&mut self, obj: Value, name_id: StringId, args: ArgValues) -> Result<CallResult, RunError> {
        let attr = EitherStr::Interned(name_id);

        match obj {
            Value::Ref(heap_id) => {
                // Check for list.sort - needs special handling for key functions
                if name_id == StaticStrings::Sort && matches!(self.heap.get(heap_id), HeapData::List(_)) {
                    let result = do_list_sort(heap_id, args, self.heap, self.interns, self.print_writer);
                    obj.drop_with_heap(self.heap);
                    return result.map(|()| CallResult::Push(Value::None));
                }
                // Instance method calls need special handling: look up the method,
                // then call it with `self` prepended.
                // Inc_ref before dropping obj so the instance stays alive during lookup.
                // call_instance_method will inc_ref again for the self_arg if needed.
                // We dec_ref after the call completes to balance this temporary hold.
                if matches!(self.heap.get(heap_id), HeapData::Instance(_)) {
                    self.heap.inc_ref(heap_id);
                    obj.drop_with_heap(self.heap);
                    let result = self.call_instance_method(heap_id, name_id, args);
                    self.heap.dec_ref(heap_id);
                    return result;
                }
                // SuperProxy method calls: look up via MRO, call with instance as self
                if matches!(self.heap.get(heap_id), HeapData::SuperProxy(_)) {
                    // Extract info before dropping (SuperProxy may have refcount 1)
                    let (instance_id, current_class_id) = match self.heap.get(heap_id) {
                        HeapData::SuperProxy(sp) => (sp.instance_id(), sp.current_class_id()),
                        _ => unreachable!(),
                    };
                    obj.drop_with_heap(self.heap);
                    return self.call_super_method_with_ids(instance_id, current_class_id, name_id, args);
                }
                // ClassObject method calls: look up in namespace, unwrap descriptors,
                // handle @staticmethod (no self/cls), @classmethod (prepend cls), regular calls.
                if matches!(self.heap.get(heap_id), HeapData::ClassObject(_)) {
                    obj.drop_with_heap(self.heap);
                    return self.call_class_method(heap_id, name_id, args);
                }
                // Call the method on the heap object using call_attr_raw to support OS/external calls
                let result = self.heap.call_attr_raw(heap_id, &attr, args, self.interns);
                obj.drop_with_heap(self.heap);
                // Convert AttrCallResult to CallResult
                result.map(Into::into)
            }
            Value::InternString(string_id) => {
                // Call string method on interned string literal using the unified dispatcher
                let s = self.interns.get_str(string_id);
                call_str_method(s, name_id, args, self.heap, self.interns).map(CallResult::Push)
            }
            Value::InternBytes(bytes_id) => {
                // Call bytes method on interned bytes literal using the unified dispatcher
                let b = self.interns.get_bytes(bytes_id);
                call_bytes_method(b, name_id, args, self.heap, self.interns).map(CallResult::Push)
            }
            Value::Builtin(Builtins::Type(t)) => {
                // Handle classmethods on type objects like dict.fromkeys()
                call_type_method(t, name_id, args, self.heap, self.interns).map(CallResult::Push)
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
    /// - `Value::ModuleFunction`: calls module function directly, returns `Push`
    /// - `Value::ExtFunction`: returns `External` for caller to execute
    /// - `Value::DefFunction`: pushes a new frame, returns `FramePushed`
    /// - `Value::Ref`: checks for closure/function on heap
    pub(super) fn call_function(&mut self, callable: Value, args: ArgValues) -> Result<CallResult, RunError> {
        match callable {
            Value::Builtin(Builtins::Function(BuiltinsFunctions::Super)) => {
                // super() needs VM context - handle it specially
                let result = self.call_super(args)?;
                Ok(CallResult::Push(result))
            }
            Value::Builtin(Builtins::Function(BuiltinsFunctions::Isinstance)) => self.call_isinstance(args),
            Value::Builtin(Builtins::Function(BuiltinsFunctions::Issubclass)) => self.call_issubclass(args),
            Value::Builtin(builtin) => {
                let result = builtin.call(self.heap, args, self.interns, self.print_writer)?;
                Ok(CallResult::Push(result))
            }
            Value::ModuleFunction(mf) => {
                let result = mf.call(self.heap, self.interns, args)?;
                Ok(result.into())
            }
            Value::ExtFunction(ext_id) => {
                // External function - return to caller to execute
                Ok(CallResult::External(ext_id, args))
            }
            Value::DefFunction(func_id) => {
                // Defined function without defaults or captured variables
                self.call_def_function(func_id, &[], Vec::new(), args)
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

    /// Calls the `isinstance()` builtin with metaclass `__instancecheck__` support.
    fn call_isinstance(&mut self, args: ArgValues) -> Result<CallResult, RunError> {
        let (obj, classinfo) = args.get_two_args("isinstance", self.heap)?;

        if let Value::Ref(class_id) = &classinfo
            && matches!(self.heap.get(*class_id), HeapData::ClassObject(_))
        {
            let dunder_id: StringId = StaticStrings::DunderInstancecheck.into();
            if let Some(method) = self.lookup_metaclass_dunder(*class_id, dunder_id) {
                let result = self.call_class_dunder(*class_id, method, ArgValues::One(obj));
                classinfo.drop_with_heap(self.heap);
                return match result {
                    Ok(CallResult::Push(value)) => {
                        let b = value.py_bool(self.heap, self.interns);
                        value.drop_with_heap(self.heap);
                        Ok(CallResult::Push(Value::Bool(b)))
                    }
                    Ok(CallResult::FramePushed) => {
                        self.pending_instancecheck_return = true;
                        Ok(CallResult::FramePushed)
                    }
                    Ok(other) => Ok(other),
                    Err(e) => Err(e),
                };
            }
        }

        // Fallback to builtin implementation
        let result = crate::builtins::isinstance::builtin_isinstance(self.heap, ArgValues::Two(obj, classinfo))?;
        Ok(CallResult::Push(result))
    }

    /// Calls the `issubclass()` builtin with metaclass `__subclasscheck__` support.
    fn call_issubclass(&mut self, args: ArgValues) -> Result<CallResult, RunError> {
        let (cls_val, classinfo) = args.get_two_args("issubclass", self.heap)?;

        if let Value::Ref(class_id) = &classinfo
            && matches!(self.heap.get(*class_id), HeapData::ClassObject(_))
        {
            let dunder_id: StringId = StaticStrings::DunderSubclasscheck.into();
            if let Some(method) = self.lookup_metaclass_dunder(*class_id, dunder_id) {
                let result = self.call_class_dunder(*class_id, method, ArgValues::One(cls_val));
                classinfo.drop_with_heap(self.heap);
                return match result {
                    Ok(CallResult::Push(value)) => {
                        let b = value.py_bool(self.heap, self.interns);
                        value.drop_with_heap(self.heap);
                        Ok(CallResult::Push(Value::Bool(b)))
                    }
                    Ok(CallResult::FramePushed) => {
                        self.pending_subclasscheck_return = true;
                        Ok(CallResult::FramePushed)
                    }
                    Ok(other) => Ok(other),
                    Err(e) => Err(e),
                };
            }
        }

        let result = crate::builtins::isinstance::builtin_issubclass(self.heap, ArgValues::Two(cls_val, classinfo))?;
        Ok(CallResult::Push(result))
    }

    /// Handles calling a heap-allocated callable (closure, function with defaults, or class).
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
        // Built-in callable wrappers for class and function introspection.
        if matches!(self.heap.get(heap_id), HeapData::ClassSubclasses(_)) {
            let class_id = match self.heap.get(heap_id) {
                HeapData::ClassSubclasses(cs) => cs.class_id(),
                _ => unreachable!(),
            };
            args.check_zero_args("type.__subclasses__", self.heap)?;
            callable.drop_with_heap(self.heap);
            let subclasses = self.collect_class_subclasses(class_id)?;
            let list_id = self.heap.allocate(HeapData::List(List::new(subclasses)))?;
            return Ok(CallResult::Push(Value::Ref(list_id)));
        }

        if matches!(self.heap.get(heap_id), HeapData::ClassGetItem(_)) {
            let class_id = match self.heap.get(heap_id) {
                HeapData::ClassGetItem(cg) => cg.class_id(),
                _ => unreachable!(),
            };
            let (first, second) = args.get_one_two_args("type.__class_getitem__", self.heap)?;
            let item = if let Some(second) = second {
                first.drop_with_heap(self.heap);
                second
            } else {
                first
            };
            self.heap.inc_ref(class_id);
            let origin = Value::Ref(class_id);
            callable.drop_with_heap(self.heap);
            let alias = make_generic_alias(origin, item, self.heap, self.interns)?;
            return Ok(CallResult::Push(alias));
        }

        if matches!(self.heap.get(heap_id), HeapData::FunctionGet(_)) {
            let func_value = match self.heap.get(heap_id) {
                HeapData::FunctionGet(getter) => getter.func().clone_with_heap(self.heap),
                _ => unreachable!(),
            };
            let (obj, owner) = args.get_one_two_args("function.__get__", self.heap)?;
            if let Some(owner) = owner {
                owner.drop_with_heap(self.heap);
            }
            callable.drop_with_heap(self.heap);
            if matches!(obj, Value::None) {
                obj.drop_with_heap(self.heap);
                return Ok(CallResult::Push(func_value));
            }
            let bound_id = self
                .heap
                .allocate(HeapData::BoundMethod(crate::types::BoundMethod::new(func_value, obj)))?;
            return Ok(CallResult::Push(Value::Ref(bound_id)));
        }

        if matches!(self.heap.get(heap_id), HeapData::WeakRef(_)) {
            args.check_zero_args("weakref", self.heap)?;
            let target_id = match self.heap.get(heap_id) {
                HeapData::WeakRef(wr) => wr.target(),
                _ => unreachable!(),
            };
            callable.drop_with_heap(self.heap);
            if let Some(target_id) = target_id {
                if self.heap.get_if_live(target_id).is_some() {
                    self.heap.inc_ref(target_id);
                    return Ok(CallResult::Push(Value::Ref(target_id)));
                }
                self.heap.with_entry_mut(heap_id, |_, data| {
                    let HeapData::WeakRef(wr) = data else {
                        return Err(RunError::internal("weakref target mutated during call"));
                    };
                    wr.clear();
                    Ok(())
                })?;
            }
            return Ok(CallResult::Push(Value::None));
        }

        // Check if this is a ClassObject (class instantiation)
        if matches!(self.heap.get(heap_id), HeapData::ClassObject(_)) {
            let call_id: StringId = StaticStrings::DunderCall.into();
            if let Some(method) = self.lookup_metaclass_dunder(heap_id, call_id) {
                let result = self.call_class_dunder(heap_id, method, args)?;
                callable.drop_with_heap(self.heap);
                return Ok(result);
            }
            return self.call_class_instantiate(heap_id, callable, args);
        }

        // Check if this is an Instance with __call__
        if matches!(self.heap.get(heap_id), HeapData::Instance(_)) {
            let dunder_id: StringId = StaticStrings::DunderCall.into();
            if let Some(method) = self.lookup_type_dunder(heap_id, dunder_id) {
                callable.drop_with_heap(self.heap);
                return self.call_dunder(heap_id, method, args);
            }
            callable.drop_with_heap(self.heap);
            args.drop_with_heap(self.heap);
            return Err(ExcType::type_error("object is not callable"));
        }

        // Check if this is a bound method (prepend bound self/cls).
        if matches!(self.heap.get(heap_id), HeapData::BoundMethod(_)) {
            let (func, self_arg) = match self.heap.get(heap_id) {
                HeapData::BoundMethod(bm) => (
                    bm.func().clone_with_heap(self.heap),
                    bm.self_arg().clone_with_heap(self.heap),
                ),
                _ => unreachable!("call_heap_callable: not a BoundMethod"),
            };
            callable.drop_with_heap(self.heap);

            let new_args = match args {
                ArgValues::Empty => ArgValues::One(self_arg),
                ArgValues::One(a) => ArgValues::Two(self_arg, a),
                ArgValues::Two(a, b) => ArgValues::ArgsKargs {
                    args: vec![self_arg, a, b],
                    kwargs: KwargsValues::Empty,
                },
                ArgValues::Kwargs(kw) => ArgValues::ArgsKargs {
                    args: vec![self_arg],
                    kwargs: kw,
                },
                ArgValues::ArgsKargs { mut args, kwargs } => {
                    args.insert(0, self_arg);
                    ArgValues::ArgsKargs { args, kwargs }
                }
            };

            return self.call_function(func, new_args);
        }

        // Check if this is a PropertyAccessor (@prop.setter / @prop.deleter / @prop.getter)
        if matches!(self.heap.get(heap_id), HeapData::PropertyAccessor(_)) {
            return self.call_property_accessor(heap_id, callable, args);
        }

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
        self.call_def_function(func_id, &cells, defaults, args)
    }

    /// Collects live direct subclasses for `type.__subclasses__()`.
    ///
    /// Prunes stale registry entries (freed or reused heap slots) to keep the
    /// subclass list accurate without holding strong references.
    fn collect_class_subclasses(&mut self, class_id: HeapId) -> RunResult<Vec<Value>> {
        let mut results = Vec::new();
        self.heap.with_entry_mut(class_id, |heap, data| {
            let HeapData::ClassObject(cls) = data else {
                return Err(ExcType::type_error(
                    "type.__subclasses__ called on non-class".to_string(),
                ));
            };

            let mut fresh: Vec<crate::types::SubclassEntry> = Vec::new();
            for entry in cls.subclasses() {
                let subclass_id = entry.class_id();
                let Some(HeapData::ClassObject(sub_cls)) = heap.get_if_live(subclass_id) else {
                    continue;
                };
                if sub_cls.class_uid() != entry.class_uid() {
                    continue;
                }
                heap.inc_ref(subclass_id);
                results.push(Value::Ref(subclass_id));
                fresh.push(*entry);
            }

            cls.set_subclasses(fresh);
            Ok(())
        })?;
        Ok(results)
    }

    /// Instantiates a class by creating an Instance and calling `__init__`.
    ///
    /// 1. Creates a new Instance on the heap referencing the ClassObject
    /// 2. Looks up `__init__` in the class namespace
    /// 3. If found, calls it with (instance, *args) and marks the frame
    ///    so that the instance is returned instead of `__init__`'s None return
    /// 4. If not found and args are provided, raises TypeError
    /// 5. If not found and no args, returns the instance directly
    fn call_class_instantiate(
        &mut self,
        class_heap_id: HeapId,
        callable: Value,
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        // Look up __new__ and __init__ via MRO.
        let new_name_id: StringId = StaticStrings::DunderNew.into();
        let init_name_id: StringId = StaticStrings::DunderInit.into();
        let new_name = self.interns.get_str(new_name_id);
        let init_name = self.interns.get_str(init_name_id);

        let (new_info, init_info) = match self.heap.get(class_heap_id) {
            HeapData::ClassObject(cls) => {
                let new_val = cls
                    .mro_lookup_attr(new_name, class_heap_id, self.heap, self.interns)
                    .map(|(v, _)| v);
                let init_val = cls
                    .mro_lookup_attr(init_name, class_heap_id, self.heap, self.interns)
                    .map(|(v, _)| v);
                (new_val, init_val)
            }
            _ => unreachable!("call_class_instantiate: not a ClassObject"),
        };

        // Drop the callable ref (we've copied what we need)
        callable.drop_with_heap(self.heap);

        // If the class defines __new__, call it first.
        // __new__ receives (cls, *args) and returns the new instance (or any value).
        if let Some(new_func) = new_info {
            // Collect original positional args into a Vec for reuse.
            // Clone each value for the __new__ call, keeping originals for __init__.
            let (orig_pos_args, orig_kwargs) = args.into_parts();
            let orig_pos: Vec<Value> = orig_pos_args.collect();
            let new_kwargs = match self.clone_kwargs_values(&orig_kwargs) {
                Ok(kwargs) => kwargs,
                Err(e) => {
                    for value in orig_pos {
                        value.drop_with_heap(self.heap);
                    }
                    orig_kwargs.drop_with_heap(self.heap);
                    return Err(e);
                }
            };

            // Build __new__ args: (cls, *cloned_args)
            self.heap.inc_ref(class_heap_id);
            let mut new_arg_list = vec![Value::Ref(class_heap_id)];
            for v in &orig_pos {
                new_arg_list.push(v.clone_with_heap(self.heap));
            }
            let new_args = ArgValues::ArgsKargs {
                args: new_arg_list,
                kwargs: new_kwargs,
            };

            // Rebuild original args from the collected positional values
            let init_args = if orig_pos.is_empty() && orig_kwargs.is_empty() {
                ArgValues::Empty
            } else {
                ArgValues::ArgsKargs {
                    args: orig_pos,
                    kwargs: orig_kwargs,
                }
            };

            let result = self.call_function(new_func, new_args)?;

            match result {
                CallResult::Push(new_result) => {
                    // __new__ completed synchronously -- check result and maybe call __init__
                    return self.handle_new_result(new_result, class_heap_id, init_info, init_args);
                }
                CallResult::FramePushed => {
                    // __new__ pushed a frame -- stash state so we can call __init__ on return
                    self.pending_new_call = Some(PendingNewCall {
                        class_heap_id,
                        init_func: init_info,
                        args: init_args,
                    });
                    return Ok(CallResult::FramePushed);
                }
                other => {
                    if let Some(init_func) = init_info {
                        init_func.drop_with_heap(self.heap);
                    }
                    return Ok(other);
                }
            }
        }

        // No __new__ -- use the standard path: create instance, call __init__.
        let instance_value = self.allocate_instance_for_class(class_heap_id)?;
        let Value::Ref(instance_heap_id) = instance_value else {
            unreachable!("allocate_instance_for_class must return heap ref");
        };

        if let Some(init_func) = init_info {
            // __init__ exists - call it with (instance, *args).
            self.heap.inc_ref(instance_heap_id);
            let init_self_arg = Value::Ref(instance_heap_id);

            // Prepend self to args
            let new_args = match args {
                ArgValues::Empty => ArgValues::One(init_self_arg),
                ArgValues::One(a) => ArgValues::Two(init_self_arg, a),
                ArgValues::Two(a, b) => ArgValues::ArgsKargs {
                    args: vec![init_self_arg, a, b],
                    kwargs: KwargsValues::Empty,
                },
                ArgValues::Kwargs(kw) => ArgValues::ArgsKargs {
                    args: vec![init_self_arg],
                    kwargs: kw,
                },
                ArgValues::ArgsKargs { mut args, kwargs } => {
                    args.insert(0, init_self_arg);
                    ArgValues::ArgsKargs { args, kwargs }
                }
            };

            let mut instance_guard = HeapGuard::new(instance_value, self);
            // Call __init__ with a guard so the instance is dropped on error paths.
            let result = {
                let this = instance_guard.heap();
                this.call_function(init_func, new_args)?
            };

            let instance_value = instance_guard.into_inner();
            match result {
                CallResult::Push(value) => {
                    // __init__ returned synchronously
                    value.drop_with_heap(self.heap);
                    Ok(CallResult::Push(instance_value))
                }
                CallResult::FramePushed => {
                    // __init__ pushed a frame - mark it so we return the instance
                    self.current_frame_mut().init_instance = Some(instance_value);
                    Ok(CallResult::FramePushed)
                }
                CallResult::External(ext_id, ext_args) => {
                    instance_value.drop_with_heap(self.heap);
                    Ok(CallResult::External(ext_id, ext_args))
                }
                CallResult::OsCall(os_func, os_args) => {
                    instance_value.drop_with_heap(self.heap);
                    Ok(CallResult::OsCall(os_func, os_args))
                }
            }
        } else {
            // No __init__ - check that no arguments were passed
            if !matches!(args, ArgValues::Empty) {
                args.drop_with_heap(self.heap);
                instance_value.drop_with_heap(self.heap);
                let class_name = match self.heap.get(class_heap_id) {
                    HeapData::ClassObject(cls) => cls.name(self.interns).to_string(),
                    _ => "object".to_string(),
                };
                return Err(ExcType::type_error(format!("{class_name}() takes no arguments")));
            }
            Ok(CallResult::Push(instance_value))
        }
    }

    /// Clones keyword arguments with proper refcount handling.
    ///
    /// This is used when a call needs to reuse kwargs across multiple invocations
    /// (e.g., `__new__` and `__init__`).
    fn clone_kwargs_values(&mut self, kwargs: &KwargsValues) -> RunResult<KwargsValues> {
        match kwargs {
            KwargsValues::Empty => Ok(KwargsValues::Empty),
            KwargsValues::Inline(pairs) => {
                let mut out = Vec::with_capacity(pairs.len());
                for (key, value) in pairs {
                    out.push((*key, value.clone_with_heap(self.heap)));
                }
                Ok(KwargsValues::Inline(out))
            }
            KwargsValues::Dict(dict) => Ok(KwargsValues::Dict(dict.clone_with_heap(self.heap, self.interns)?)),
        }
    }

    /// Allocates a new instance for a class, honoring `__slots__` layout.
    fn allocate_instance_for_class(&mut self, class_heap_id: HeapId) -> RunResult<Value> {
        let (slot_len, has_dict, _has_weakref) = match self.heap.get(class_heap_id) {
            HeapData::ClassObject(cls) => (
                cls.slot_layout().len(),
                cls.instance_has_dict(),
                cls.instance_has_weakref(),
            ),
            _ => return Err(ExcType::type_error("object is not a class".to_string())),
        };

        self.heap.inc_ref(class_heap_id);
        let attrs_id = if has_dict {
            Some(self.heap.allocate(HeapData::Dict(Dict::new()))?)
        } else {
            None
        };
        let mut slot_values = Vec::with_capacity(slot_len);
        slot_values.resize_with(slot_len, || Value::Undefined);
        let weakref_ids = Vec::new();
        let instance = Instance::new(class_heap_id, attrs_id, slot_values, weakref_ids);
        let instance_heap_id = self.heap.allocate(HeapData::Instance(instance))?;
        Ok(Value::Ref(instance_heap_id))
    }

    /// Handles the result of a `__new__` call.
    ///
    /// If the result is an instance of the target class and `__init__` exists,
    /// calls `__init__` on the result. Otherwise, returns the result directly.
    pub(super) fn handle_new_result(
        &mut self,
        new_result: Value,
        class_heap_id: HeapId,
        init_info: Option<Value>,
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        // Check if the result is an instance of the class.
        // If __new__ returned a non-instance or an instance of a different class,
        // we skip __init__.
        let is_instance_of_class = if let Value::Ref(result_id) = &new_result {
            match self.heap.get(*result_id) {
                HeapData::Instance(inst) => inst.class_id() == class_heap_id,
                _ => false,
            }
        } else {
            false
        };

        if is_instance_of_class {
            if let Some(init_func) = init_info {
                // Call __init__ on the instance returned by __new__
                let instance_id = match &new_result {
                    Value::Ref(id) => *id,
                    _ => unreachable!(),
                };
                self.heap.inc_ref(instance_id);
                let init_self_arg = Value::Ref(instance_id);

                let new_args = match args {
                    ArgValues::Empty => ArgValues::One(init_self_arg),
                    ArgValues::One(a) => ArgValues::Two(init_self_arg, a),
                    ArgValues::Two(a, b) => ArgValues::ArgsKargs {
                        args: vec![init_self_arg, a, b],
                        kwargs: KwargsValues::Empty,
                    },
                    ArgValues::Kwargs(kw) => ArgValues::ArgsKargs {
                        args: vec![init_self_arg],
                        kwargs: kw,
                    },
                    ArgValues::ArgsKargs { mut args, kwargs } => {
                        args.insert(0, init_self_arg);
                        ArgValues::ArgsKargs { args, kwargs }
                    }
                };

                let mut new_result_guard = HeapGuard::new(new_result, self);
                let result = {
                    let this = new_result_guard.heap();
                    this.call_function(init_func, new_args)?
                };
                let new_result = new_result_guard.into_inner();

                match result {
                    CallResult::Push(value) => {
                        value.drop_with_heap(self.heap);
                        Ok(CallResult::Push(new_result))
                    }
                    CallResult::FramePushed => {
                        self.current_frame_mut().init_instance = Some(new_result);
                        Ok(CallResult::FramePushed)
                    }
                    CallResult::External(ext_id, ext_args) => {
                        new_result.drop_with_heap(self.heap);
                        Ok(CallResult::External(ext_id, ext_args))
                    }
                    CallResult::OsCall(os_func, os_args) => {
                        new_result.drop_with_heap(self.heap);
                        Ok(CallResult::OsCall(os_func, os_args))
                    }
                }
            } else {
                // No __init__ -- return the instance from __new__
                args.drop_with_heap(self.heap);
                Ok(CallResult::Push(new_result))
            }
        } else {
            // __new__ returned a non-instance or different class -- skip __init__
            if let Some(init_func) = init_info {
                init_func.drop_with_heap(self.heap);
            }
            args.drop_with_heap(self.heap);
            Ok(CallResult::Push(new_result))
        }
    }

    /// Calls a PropertyAccessor, creating a new UserProperty with the appropriate
    /// function slot replaced.
    ///
    /// For `@prop.setter`, calling the accessor with a function creates a new property
    /// that inherits the original getter/deleter but uses the new function as setter.
    fn call_property_accessor(
        &mut self,
        accessor_heap_id: HeapId,
        callable: Value,
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        // Get the function argument (the decorated function)
        let ArgValues::One(new_func) = args else {
            args.drop_with_heap(self.heap);
            callable.drop_with_heap(self.heap);
            return Err(ExcType::type_error("property accessor takes exactly 1 argument"));
        };

        // Phase 1: Extract data from the accessor without heap mutation
        let (kind, fget, fset, fdel) = match self.heap.get(accessor_heap_id) {
            HeapData::PropertyAccessor(acc) => {
                let (fg, fs, fd) = acc.parts();
                (
                    acc.kind(),
                    fg.map(Value::copy_for_extend),
                    fs.map(Value::copy_for_extend),
                    fd.map(Value::copy_for_extend),
                )
            }
            _ => unreachable!("call_property_accessor: not a PropertyAccessor"),
        };

        // Phase 2: Increment refcounts for the copied values
        if let Some(Value::Ref(id)) = &fget {
            self.heap.inc_ref(*id);
        }
        if let Some(Value::Ref(id)) = &fset {
            self.heap.inc_ref(*id);
        }
        if let Some(Value::Ref(id)) = &fdel {
            self.heap.inc_ref(*id);
        }

        // Drop the accessor callable (we've copied what we need)
        callable.drop_with_heap(self.heap);

        // Create a new UserProperty with the appropriate slot replaced
        let new_property = match kind {
            PropertyAccessorKind::Getter => {
                // Replace fget with new_func, drop old fget
                if let Some(old) = fget {
                    old.drop_with_heap(self.heap);
                }
                UserProperty::new(Some(new_func))
            }
            PropertyAccessorKind::Setter => {
                // Replace fset with new_func, drop old fset
                if let Some(old) = fset {
                    old.drop_with_heap(self.heap);
                }
                UserProperty::with_setter(fget, new_func)
            }
            PropertyAccessorKind::Deleter => {
                // Replace fdel with new_func, drop old fdel
                if let Some(old) = fdel {
                    old.drop_with_heap(self.heap);
                }
                UserProperty::with_deleter(fget, fset, new_func)
            }
        };

        let prop_id = self.heap.allocate(HeapData::UserProperty(new_property))?;
        Ok(CallResult::Push(Value::Ref(prop_id)))
    }

    /// Implements `super()` with no arguments (PEP 3135).
    ///
    /// Uses the `__class__` cell from the current frame to build a `SuperProxy`
    /// that delegates attribute lookup to the next class in the MRO.
    fn call_super(&mut self, args: ArgValues) -> Result<Value, RunError> {
        // super() takes no arguments in the zero-argument form
        if !matches!(args, ArgValues::Empty) {
            args.drop_with_heap(self.heap);
            return Err(ExcType::type_error(
                "super() with arguments is not supported; use super() with no arguments".to_string(),
            ));
        }

        if let Some((instance_id, defining_class_id)) = self.super_context_from_classcell()? {
            self.heap.inc_ref(instance_id);
            self.heap.inc_ref(defining_class_id);

            let proxy = crate::types::SuperProxy::new(instance_id, defining_class_id);
            let heap_id = self.heap.allocate(HeapData::SuperProxy(proxy))?;
            return Ok(Value::Ref(heap_id));
        }
        Err(ExcType::type_error("super(): __class__ cell not found".to_string()))
    }

    /// Attempts to resolve zero-argument super() context from the `__class__` cell.
    ///
    /// Returns `(instance_id, defining_class_id)` when the current frame has a
    /// `__class__` cell and a valid first local (`self`/`cls`).
    fn super_context_from_classcell(&mut self) -> Result<Option<(HeapId, HeapId)>, RunError> {
        let (class_cell_id, namespace_idx) = {
            let frame = self.current_frame();
            if frame.class_body_info.is_some() || frame.function_id.is_none() {
                return Ok(None);
            }

            let class_name_id: StringId = StaticStrings::DunderClass.into();
            let mut class_cell_id = None;
            for (idx, cell_id) in frame.cells.iter().enumerate() {
                let slot = u16::try_from(idx).expect("cell index exceeds u16");
                if frame.code.local_name(slot) == Some(class_name_id) {
                    class_cell_id = Some(*cell_id);
                    break;
                }
            }

            let Some(class_cell_id) = class_cell_id else {
                return Ok(None);
            };

            (class_cell_id, frame.namespace_idx)
        };

        let class_val = self.heap.get_cell_value(class_cell_id);
        let class_id = match class_val {
            Value::Ref(id) => {
                class_val.drop_with_heap(self.heap);
                id
            }
            other => {
                other.drop_with_heap(self.heap);
                return Err(ExcType::type_error(
                    "super(): __class__ cell is not a class".to_string(),
                ));
            }
        };

        if !matches!(self.heap.get(class_id), HeapData::ClassObject(_)) {
            return Err(ExcType::type_error(
                "super(): __class__ cell is not a class".to_string(),
            ));
        }

        let namespace = self.namespaces.get(namespace_idx);
        let first_local = namespace.get(crate::namespace::NamespaceId::new(0));
        let instance_id = match first_local {
            Value::Ref(id) if matches!(self.heap.get(*id), HeapData::Instance(_) | HeapData::ClassObject(_)) => *id,
            _ => return Err(ExcType::type_error("super(): __self__ is not an instance".to_string())),
        };

        Ok(Some((instance_id, class_id)))
    }

    /// Extracts a string from a Value (for getattr/setattr/hasattr builtin name argument).
    ///
    /// Returns the string content. Works with InternString values and heap Str values.
    fn extract_attr_name_str(&self, name_val: &Value) -> Result<String, RunError> {
        match name_val {
            Value::InternString(sid) => Ok(self.interns.get_str(*sid).to_owned()),
            Value::Ref(id) => match self.heap.get(*id) {
                HeapData::Str(s) => Ok(s.as_str().to_owned()),
                _ => Err(ExcType::type_error("attribute name must be string".to_string())),
            },
            _ => Err(ExcType::type_error("attribute name must be string".to_string())),
        }
    }

    /// Tries to convert a string to a StringId via StaticStrings lookup.
    ///
    /// Returns Some(StringId) if the name matches a known static string, None otherwise.
    fn try_static_string_id(name: &str) -> Option<StringId> {
        StaticStrings::from_str(name).ok().map(std::convert::Into::into)
    }

    /// Implementation of `getattr(obj, name[, default])` builtin.
    ///
    /// Gets an attribute by dynamic string name. Handles both static (interned)
    /// and dynamic (heap) attribute names.
    fn builtin_getattr(&mut self, args: ArgValues) -> Result<Value, RunError> {
        let (obj, name_val, default) = match args {
            ArgValues::Two(a, b) => (a, b, None),
            ArgValues::ArgsKargs { mut args, kwargs } => {
                kwargs.drop_with_heap(self.heap);
                if args.len() == 3 {
                    let c = args.remove(2);
                    let b = args.remove(1);
                    let a = args.remove(0);
                    (a, b, Some(c))
                } else if args.len() == 2 {
                    let b = args.remove(1);
                    let a = args.remove(0);
                    (a, b, None)
                } else {
                    for arg in args {
                        arg.drop_with_heap(self.heap);
                    }
                    return Err(ExcType::type_error("getattr expected 2 or 3 arguments".to_string()));
                }
            }
            other => {
                other.drop_with_heap(self.heap);
                return Err(ExcType::type_error("getattr expected 2 or 3 arguments".to_string()));
            }
        };

        let attr_name = match self.extract_attr_name_str(&name_val) {
            Ok(s) => s,
            Err(e) => {
                obj.drop_with_heap(self.heap);
                name_val.drop_with_heap(self.heap);
                if let Some(d) = default {
                    d.drop_with_heap(self.heap);
                }
                return Err(e);
            }
        };

        // Try static string path first
        let result = if let Some(sid) = Self::try_static_string_id(&attr_name) {
            obj.py_getattr(sid, self.heap, self.interns)
        } else {
            // Dynamic string: do Instance/ClassObject string-based lookup
            self.getattr_dynamic_str(&obj, &attr_name)
        };

        name_val.drop_with_heap(self.heap);
        obj.drop_with_heap(self.heap);

        match result {
            Ok(AttrCallResult::Value(val)) => {
                if let Some(d) = default {
                    d.drop_with_heap(self.heap);
                }
                Ok(val)
            }
            Ok(AttrCallResult::DescriptorGet(descriptor)) => {
                if let Some(d) = default {
                    d.drop_with_heap(self.heap);
                }
                // For getattr with a descriptor, call descriptor.__get__(None, None)
                // since obj has already been dropped.
                let get_id: StringId = StaticStrings::DunderDescGet.into();
                if let Value::Ref(desc_id) = &descriptor {
                    let desc_id = *desc_id;
                    if let Some(method) = self.lookup_type_dunder(desc_id, get_id) {
                        self.heap.inc_ref(desc_id);
                        let args = ArgValues::ArgsKargs {
                            args: vec![Value::Ref(desc_id), Value::None, Value::None],
                            kwargs: KwargsValues::Empty,
                        };
                        descriptor.drop_with_heap(self.heap);
                        let result = self.call_function(method, args)?;
                        return match result {
                            CallResult::Push(val) => Ok(val),
                            _ => Ok(Value::None),
                        };
                    }
                }
                // No __get__ found, return descriptor itself
                Ok(descriptor)
            }
            Ok(
                AttrCallResult::ExternalCall(_, _) | AttrCallResult::OsCall(_, _) | AttrCallResult::PropertyCall(_, _),
            ) => {
                if let Some(d) = default {
                    d.drop_with_heap(self.heap);
                }
                // External/OS/Property calls are not expected from getattr - treat as found
                Ok(Value::None)
            }
            Err(_) if default.is_some() => Ok(default.expect("checked above")),
            Err(e) => {
                if let Some(d) = default {
                    d.drop_with_heap(self.heap);
                }
                Err(e)
            }
        }
    }

    /// Gets an attribute by dynamic (non-interned) string name.
    ///
    /// Performs string-based lookup in Instance attrs and class MRO.
    fn getattr_dynamic_str(&mut self, obj: &Value, name: &str) -> Result<AttrCallResult, RunError> {
        if let Value::Ref(heap_id) = obj {
            let heap_id = *heap_id;
            let interns = self.interns;
            match self.heap.get(heap_id) {
                HeapData::Instance(_) => {
                    // Use with_entry_mut to safely borrow instance + heap
                    let result: Result<Option<Value>, RunError> = self.heap.with_entry_mut(heap_id, |heap, data| {
                        if let HeapData::Instance(inst) = data {
                            if name == "__dict__" {
                                let has_dict = match heap.get(inst.class_id()) {
                                    HeapData::ClassObject(cls) => cls.instance_has_dict(),
                                    _ => false,
                                };
                                if !has_dict {
                                    let class_name = match heap.get(inst.class_id()) {
                                        HeapData::ClassObject(cls) => cls.name(interns).to_string(),
                                        _ => "<unknown>".to_string(),
                                    };
                                    return Err(ExcType::attribute_error(format!("'{class_name}' object"), "__dict__"));
                                }
                                let Some(attrs_id) = inst.attrs_id() else {
                                    return Err(ExcType::attribute_error("instance", "__dict__"));
                                };
                                heap.inc_ref(attrs_id);
                                return Ok(Some(Value::Ref(attrs_id)));
                            }
                            // 1. Instance attrs
                            if let Some(dict) = inst.attrs(heap)
                                && let Some(value) = dict.get_by_str(name, heap, interns)
                            {
                                return Ok(Some(value.clone_with_heap(heap)));
                            }
                            // 2. Instance slots
                            if let Some(value) = inst.slot_value(name, heap) {
                                return Ok(Some(value.clone_with_heap(heap)));
                            }
                            // 3. Class MRO lookup
                            let class_id = inst.class_id();
                            if let HeapData::ClassObject(cls) = heap.get(class_id)
                                && let Some((value, _)) = cls.mro_lookup_attr(name, class_id, heap, interns)
                            {
                                return Ok(Some(value));
                            }
                            Ok(None)
                        } else {
                            Ok(None)
                        }
                    });
                    match result? {
                        Some(value) => Ok(AttrCallResult::Value(value)),
                        None => Err(ExcType::attribute_error(Type::Instance, name)),
                    }
                }
                HeapData::ClassObject(_) => {
                    let result: Result<Option<Value>, RunError> = self.heap.with_entry_mut(heap_id, |heap, data| {
                        if let HeapData::ClassObject(cls) = data {
                            if name == "__dict__" {
                                heap.inc_ref(heap_id);
                                let proxy_id =
                                    heap.allocate(HeapData::MappingProxy(crate::types::MappingProxy::new(heap_id)))?;
                                return Ok(Some(Value::Ref(proxy_id)));
                            }
                            if let Some(value) = cls.namespace().get_by_str(name, heap, interns) {
                                Ok(Some(value.clone_with_heap(heap)))
                            } else {
                                Ok(None)
                            }
                        } else {
                            Ok(None)
                        }
                    });
                    match result? {
                        Some(value) => Ok(AttrCallResult::Value(value)),
                        None => Err(ExcType::attribute_error(Type::Type, name)),
                    }
                }
                _ => {
                    let type_name = self.heap.get(heap_id).py_type(self.heap);
                    Err(ExcType::attribute_error(type_name, name))
                }
            }
        } else {
            let type_name = obj.py_type(self.heap);
            Err(ExcType::attribute_error(type_name, name))
        }
    }

    /// Implementation of `setattr(obj, name, value)` builtin.
    ///
    /// Sets an attribute by dynamic string name.
    fn builtin_setattr(&mut self, args: ArgValues) -> Result<Value, RunError> {
        // Extract 3 arguments (3+ args become ArgsKargs)
        let (obj, name_val, value) = match args {
            ArgValues::ArgsKargs { mut args, kwargs } => {
                kwargs.drop_with_heap(self.heap);
                if args.len() == 3 {
                    let c = args.remove(2);
                    let b = args.remove(1);
                    let a = args.remove(0);
                    (a, b, c)
                } else {
                    for arg in args {
                        arg.drop_with_heap(self.heap);
                    }
                    return Err(ExcType::type_error("setattr expected 3 arguments".to_string()));
                }
            }
            other => {
                other.drop_with_heap(self.heap);
                return Err(ExcType::type_error("setattr expected 3 arguments".to_string()));
            }
        };

        let attr_name = match self.extract_attr_name_str(&name_val) {
            Ok(s) => s,
            Err(e) => {
                obj.drop_with_heap(self.heap);
                name_val.drop_with_heap(self.heap);
                value.drop_with_heap(self.heap);
                return Err(e);
            }
        };
        name_val.drop_with_heap(self.heap);

        // Try static string path first
        if let Some(sid) = Self::try_static_string_id(&attr_name) {
            obj.py_set_attr(sid, value, self.heap, self.interns)?;
        } else {
            // Dynamic string: create heap Str key
            self.setattr_dynamic_str(&obj, &attr_name, value)?;
        }
        obj.drop_with_heap(self.heap);
        Ok(Value::None)
    }

    /// Sets an attribute by dynamic (non-interned) string name.
    fn setattr_dynamic_str(&mut self, obj: &Value, name: &str, value: Value) -> Result<(), RunError> {
        if let Value::Ref(heap_id) = obj {
            let heap_id = *heap_id;
            let is_instance = matches!(self.heap.get(heap_id), HeapData::Instance(_));
            let is_class = matches!(self.heap.get(heap_id), HeapData::ClassObject(_));
            let interns = self.interns;

            if is_instance || is_class {
                let key_id = self
                    .heap
                    .allocate(HeapData::Str(crate::types::Str::from(name.to_owned())))?;
                let name_value = Value::Ref(key_id);
                self.heap.with_entry_mut(heap_id, |heap, data| {
                    if let HeapData::Instance(inst) = data {
                        match inst.set_attr(name_value, value, heap, interns) {
                            Ok(old) => {
                                if let Some(old) = old {
                                    old.drop_with_heap(heap);
                                }
                                Ok(())
                            }
                            Err(e) => Err(e),
                        }
                    } else if let HeapData::ClassObject(cls) = data {
                        match cls.set_attr(name_value, value, heap, interns) {
                            Ok(old) => {
                                if let Some(old) = old {
                                    old.drop_with_heap(heap);
                                }
                                Ok(())
                            }
                            Err(e) => Err(e),
                        }
                    } else {
                        unreachable!("type changed during borrow")
                    }
                })
            } else {
                let type_name = self.heap.get(heap_id).py_type(self.heap);
                value.drop_with_heap(self.heap);
                Err(ExcType::attribute_error_no_setattr(type_name, name))
            }
        } else {
            let type_name = obj.py_type(self.heap);
            value.drop_with_heap(self.heap);
            Err(ExcType::attribute_error_no_setattr(type_name, name))
        }
    }

    /// Implementation of `hasattr(obj, name)` builtin.
    ///
    /// Returns True if the object has the named attribute, False otherwise.
    fn builtin_hasattr(&mut self, args: ArgValues) -> Result<Value, RunError> {
        let (obj, name_val) = args.get_two_args("hasattr", self.heap)?;

        let attr_name = match self.extract_attr_name_str(&name_val) {
            Ok(s) => s,
            Err(e) => {
                obj.drop_with_heap(self.heap);
                name_val.drop_with_heap(self.heap);
                return Err(e);
            }
        };
        name_val.drop_with_heap(self.heap);

        let result = if let Some(sid) = Self::try_static_string_id(&attr_name) {
            obj.py_getattr(sid, self.heap, self.interns)
        } else {
            self.getattr_dynamic_str(&obj, &attr_name)
        };

        obj.drop_with_heap(self.heap);

        match result {
            Ok(AttrCallResult::Value(val)) => {
                val.drop_with_heap(self.heap);
                Ok(Value::Bool(true))
            }
            Ok(AttrCallResult::DescriptorGet(desc)) => {
                desc.drop_with_heap(self.heap);
                Ok(Value::Bool(true))
            }
            Ok(_) => Ok(Value::Bool(true)),
            Err(_) => Ok(Value::Bool(false)),
        }
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
        let copied_args = self.extract_args_tuple(&args_tuple);

        // Increment refcounts for positional args
        for arg in &copied_args {
            if let Value::Ref(id) = arg {
                self.heap.inc_ref(*id);
            }
        }

        // Build ArgValues from positional args and optional kwargs
        let args = if let Some(kwargs_ref) = kwargs {
            self.build_args_with_kwargs(copied_args, kwargs_ref)?
        } else {
            Self::build_args_positional_only(copied_args)
        };

        // Clean up the args tuple ref (we cloned the contents)
        args_tuple.drop_with_heap(self.heap);

        // Call the function
        self.call_function(callable, args)
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
        // Extract positional args from tuple
        let copied_args = self.extract_args_tuple_for_attr(&args_tuple);

        // Increment refcounts for positional args
        for arg in &copied_args {
            if let Value::Ref(id) = arg {
                self.heap.inc_ref(*id);
            }
        }

        // Build ArgValues from positional args and optional kwargs
        let args = if let Some(kwargs_ref) = kwargs {
            self.build_args_with_kwargs_for_attr(copied_args, kwargs_ref)?
        } else {
            Self::build_args_positional_only(copied_args)
        };

        // Clean up the args tuple ref (we cloned the contents)
        args_tuple.drop_with_heap(self.heap);

        // Call the method
        self.call_attr(obj, name_id, args)
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
        tuple.as_vec().iter().map(Value::copy_for_extend).collect()
    }

    /// Builds `ArgValues` with kwargs for `CallFunctionExtended`.
    ///
    /// # Panics
    /// Panics if `kwargs_ref` is not a dict. This indicates a compiler bug since
    /// the compiler always emits `BuildDict` before `CallFunctionExtended` with kwargs.
    fn build_args_with_kwargs(&mut self, copied_args: Vec<Value>, kwargs_ref: Value) -> Result<ArgValues, RunError> {
        // Extract kwargs dict items
        let Value::Ref(id) = &kwargs_ref else {
            unreachable!("CallFunctionExtended: kwargs must be a Ref")
        };
        let HeapData::Dict(dict) = self.heap.get(*id) else {
            unreachable!("CallFunctionExtended: kwargs must be a Dict")
        };
        let copied_kwargs: Vec<(Value, Value)> = dict
            .iter()
            .map(|(k, v)| (Value::copy_for_extend(k), Value::copy_for_extend(v)))
            .collect();

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
        tuple.as_vec().iter().map(Value::copy_for_extend).collect()
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
        // Extract kwargs dict items
        let Value::Ref(id) = &kwargs_ref else {
            unreachable!("CallAttrExtended: kwargs must be a Ref")
        };
        let HeapData::Dict(dict) = self.heap.get(*id) else {
            unreachable!("CallAttrExtended: kwargs must be a Dict")
        };
        let copied_kwargs: Vec<(Value, Value)> = dict
            .iter()
            .map(|(k, v)| (Value::copy_for_extend(k), Value::copy_for_extend(v)))
            .collect();

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
        defaults: Vec<Value>,
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        // Get function info (interns is a shared reference so no conflict)
        let func = self.interns.get_function(func_id);

        if func.is_async {
            // Async function: create a Coroutine instead of pushing a frame
            self.create_coroutine(func_id, cells, defaults, args)
        } else {
            // Sync function: push a new frame
            self.call_sync_function(func_id, cells, defaults, args)
        }
    }

    /// Creates a Coroutine for an async function call.
    ///
    /// Binds arguments immediately (errors are raised at call time, not await time)
    /// but stores the namespace in the Coroutine instead of registering it.
    /// The coroutine is executed when awaited via Await.
    fn create_coroutine(
        &mut self,
        func_id: FunctionId,
        cells: &[HeapId],
        defaults: Vec<Value>,
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        let func = self.interns.get_function(func_id);

        // 1. Create namespace vector (not registered with Namespaces)
        let mut namespace = Vec::with_capacity(func.namespace_size);

        // 2. Bind arguments to parameters
        {
            let bind_result = func
                .signature
                .bind(args, &defaults, self.heap, self.interns, func.name, &mut namespace);

            if let Err(e) = bind_result {
                // Clean up namespace values on error
                for value in namespace {
                    value.drop_with_heap(self.heap);
                }
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

        // Track created cell HeapIds for the coroutine
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

        // 6. Create Coroutine on heap
        let coroutine = Coroutine::new(func_id, namespace, frame_cells);
        let coroutine_id = self.heap.allocate(HeapData::Coroutine(coroutine))?;

        Ok(CallResult::Push(Value::Ref(coroutine_id)))
    }

    /// Calls a sync function by pushing a new frame.
    ///
    /// Sets up the function's namespace with bound arguments, cell variables,
    /// and free variables (captured from enclosing scope for closures).
    fn call_sync_function(
        &mut self,
        func_id: FunctionId,
        cells: &[HeapId],
        defaults: Vec<Value>,
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        // Get call position BEFORE borrowing namespaces mutably
        let call_position = self.current_position();

        // Get function info (interns is a shared reference so no conflict)
        let func = self.interns.get_function(func_id);

        // 1. Create new namespace for function
        let namespace_idx = match self.namespaces.new_namespace(func.namespace_size, self.heap) {
            Ok(idx) => idx,
            Err(e) => {
                // Ensure args/defaults are cleaned up on early recursion/memory errors.
                args.drop_with_heap(self.heap);
                for default in defaults {
                    default.drop_with_heap(self.heap);
                }
                return Err(e.into());
            }
        };

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
            Some(call_position),
        ));

        Ok(CallResult::FramePushed)
    }

    /// Calls a method on an Instance.
    ///
    /// Looks up the method in the instance's class namespace (instance attrs first,
    /// then class attrs). If the found attribute is a callable (function/closure),
    /// calls it with `self` (the instance) prepended to the arguments.
    fn call_instance_method(
        &mut self,
        instance_heap_id: HeapId,
        method_name_id: StringId,
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        let method_name = self.interns.get_str(method_name_id);

        // Phase 1: Look up the method value with proper refcount handling.
        // Check instance attrs first, then class attrs.
        // We track where the value was found: instance attrs don't get auto-bound (no self
        // prepended), while class attrs do (they are unbound methods that need self).
        // This matches Python semantics: functions in instance.__dict__ are plain callables,
        // only functions found on the class are auto-bound as methods.
        let method_lookup: Result<Option<(Value, bool)>, _> =
            self.heap.with_entry_mut(instance_heap_id, |heap, data| {
                let HeapData::Instance(inst) = data else {
                    unreachable!("call_instance_method: not an Instance");
                };

                // 1. Check instance attributes (found_on_instance = true)
                if let Some(dict) = inst.attrs(heap)
                    && let Some(value) = dict.get_by_str(method_name, heap, self.interns)
                {
                    return Ok(Some((value.clone_with_heap(heap), true)));
                }
                if let Some(value) = inst.slot_value(method_name, heap) {
                    return Ok(Some((value.clone_with_heap(heap), true)));
                }

                // 2. Check class attributes via MRO (found_on_instance = false)
                match heap.get(inst.class_id()) {
                    HeapData::ClassObject(cls) => {
                        if let Some((value, _found_in)) =
                            cls.mro_lookup_attr(method_name, inst.class_id(), heap, self.interns)
                        {
                            Ok(Some((value, false)))
                        } else {
                            let class_name = cls.name(self.interns).to_string();
                            Err(ExcType::attribute_error(format!("'{class_name}' object"), method_name))
                        }
                    }
                    _ => Err(ExcType::attribute_error(Type::Instance, method_name)),
                }
            });

        let (method_value, found_on_instance) = match method_lookup {
            Ok(Some(v)) => v,
            Ok(None) => unreachable!("should not happen"),
            Err(e) => {
                // Note: the caller (call_attr) already dropped obj before calling us,
                // so we don't need to dec_ref the instance here.
                args.drop_with_heap(self.heap);
                return Err(e);
            }
        };

        // If found on instance dict, call directly without binding (no self prepend).
        // In Python, functions stored in instance.__dict__ are plain callables.
        if found_on_instance {
            return self.call_function(method_value, args);
        }

        // Phase 2: Found on class -- check for descriptor wrappers and unwrap if needed.
        // StaticMethod -> call inner func directly (no self/cls)
        // ClassMethod -> call inner func with cls as first arg
        // Other -> normal method call (prepend self)
        #[expect(clippy::items_after_statements)]
        /// Describes how a resolved instance attribute should be invoked.
        ///
        /// This captures the unwrapped callable and the binding strategy that
        /// matches Python's descriptor rules for instance lookups.
        enum InstanceCallKind {
            StaticMethod(Value), // Inner func, no self/cls
            ClassMethod(Value),  // Inner func, prepend cls
            Normal(Value),       // Regular method, prepend self
        }

        let call_kind = if let Value::Ref(ref_id) = &method_value {
            let ref_id = *ref_id;
            match self.heap.get(ref_id) {
                HeapData::StaticMethod(sm) => {
                    let func = sm.func().clone_with_heap(self.heap);
                    method_value.drop_with_heap(self.heap);
                    InstanceCallKind::StaticMethod(func)
                }
                HeapData::ClassMethod(cm) => {
                    let func = cm.func().clone_with_heap(self.heap);
                    method_value.drop_with_heap(self.heap);
                    InstanceCallKind::ClassMethod(func)
                }
                _ => InstanceCallKind::Normal(method_value),
            }
        } else {
            InstanceCallKind::Normal(method_value)
        };

        match call_kind {
            InstanceCallKind::StaticMethod(func) => {
                // StaticMethod: call the inner function directly, no self/cls
                self.call_function(func, args)
            }
            InstanceCallKind::ClassMethod(func) => {
                // ClassMethod: prepend the class as first arg (cls)
                // Get the class_id from the instance
                let class_id = match self.heap.get(instance_heap_id) {
                    HeapData::Instance(inst) => inst.class_id(),
                    _ => unreachable!(),
                };
                self.heap.inc_ref(class_id);
                let cls_arg = Value::Ref(class_id);
                let new_args = match args {
                    ArgValues::Empty => ArgValues::One(cls_arg),
                    ArgValues::One(a) => ArgValues::Two(cls_arg, a),
                    ArgValues::Two(a, b) => ArgValues::ArgsKargs {
                        args: vec![cls_arg, a, b],
                        kwargs: KwargsValues::Empty,
                    },
                    ArgValues::Kwargs(kw) => ArgValues::ArgsKargs {
                        args: vec![cls_arg],
                        kwargs: kw,
                    },
                    ArgValues::ArgsKargs { mut args, kwargs } => {
                        args.insert(0, cls_arg);
                        ArgValues::ArgsKargs { args, kwargs }
                    }
                };
                self.call_function(func, new_args)
            }
            InstanceCallKind::Normal(method_value) => {
                // Regular method: prepend instance as self argument.
                let is_callable = matches!(
                    method_value,
                    Value::DefFunction(_)
                        | Value::Ref(_)
                        | Value::Builtin(_)
                        | Value::ModuleFunction(_)
                        | Value::ExtFunction(_)
                );

                if is_callable {
                    self.heap.inc_ref(instance_heap_id);
                    let self_arg = Value::Ref(instance_heap_id);

                    let new_args = match args {
                        ArgValues::Empty => ArgValues::One(self_arg),
                        ArgValues::One(a) => ArgValues::Two(self_arg, a),
                        ArgValues::Two(a, b) => ArgValues::ArgsKargs {
                            args: vec![self_arg, a, b],
                            kwargs: KwargsValues::Empty,
                        },
                        ArgValues::Kwargs(kw) => ArgValues::ArgsKargs {
                            args: vec![self_arg],
                            kwargs: kw,
                        },
                        ArgValues::ArgsKargs { mut args, kwargs } => {
                            args.insert(0, self_arg);
                            ArgValues::ArgsKargs { args, kwargs }
                        }
                    };

                    self.call_function(method_value, new_args)
                } else {
                    // Not callable - report error.
                    args.drop_with_heap(self.heap);
                    method_value.drop_with_heap(self.heap);
                    Err(ExcType::type_error("attribute is not callable"))
                }
            }
        }
    }

    /// Calls a method on a class object.
    ///
    /// Looks up the attribute in the class namespace (with MRO), then:
    /// - StaticMethod: calls the inner function directly (no self/cls)
    /// - ClassMethod: calls the inner function with the class as first arg
    /// - Regular function: calls directly (no self prepended)
    fn call_class_method(
        &mut self,
        class_heap_id: HeapId,
        method_name_id: StringId,
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        let method_name = self.interns.get_str(method_name_id);
        let interns = self.interns;

        // Phase 1: Look up the attribute and determine its descriptor type.
        #[expect(clippy::items_after_statements)]
        /// Descriptor outcome for a class-level attribute lookup.
        ///
        /// Used to decide whether to bind a classmethod, bypass a staticmethod,
        /// or call the value directly.
        enum DescriptorKind {
            StaticMethod(Value), // Inner function (no self/cls binding)
            ClassMethod(Value),  // Inner function (prepend cls)
            Regular(Value),      // Regular value (call directly)
        }

        let lookup_result = self.heap.with_entry_mut(class_heap_id, |heap, data| {
            let HeapData::ClassObject(cls) = data else {
                unreachable!("call_class_method: not a ClassObject");
            };

            // Look up in own namespace + MRO
            if let Some((value, _found_in)) = cls.mro_lookup_attr(method_name, class_heap_id, heap, interns) {
                // Check descriptor type
                if let Value::Ref(id) = &value {
                    let id = *id;
                    match heap.get(id) {
                        HeapData::StaticMethod(sm) => {
                            let func = sm.func().clone_with_heap(heap);
                            value.drop_with_heap(heap);
                            return Ok(DescriptorKind::StaticMethod(func));
                        }
                        HeapData::ClassMethod(cm) => {
                            let func = cm.func().clone_with_heap(heap);
                            value.drop_with_heap(heap);
                            return Ok(DescriptorKind::ClassMethod(func));
                        }
                        _ => {}
                    }
                }
                Ok(DescriptorKind::Regular(value))
            } else {
                let class_name = cls.name(interns).to_string();
                Err(ExcType::attribute_error(
                    format!("type object '{class_name}'"),
                    method_name,
                ))
            }
        });

        let descriptor = match lookup_result {
            Ok(d) => d,
            Err(e) => {
                args.drop_with_heap(self.heap);
                return Err(e);
            }
        };

        // Phase 2: Call the resolved descriptor
        match descriptor {
            DescriptorKind::StaticMethod(func) => {
                // StaticMethod: call the inner function directly, no self/cls
                self.call_function(func, args)
            }
            DescriptorKind::ClassMethod(func) => {
                // ClassMethod: prepend the class as first arg (cls)
                self.heap.inc_ref(class_heap_id);
                let cls_arg = Value::Ref(class_heap_id);
                let new_args = match args {
                    ArgValues::Empty => ArgValues::One(cls_arg),
                    ArgValues::One(a) => ArgValues::Two(cls_arg, a),
                    ArgValues::Two(a, b) => ArgValues::ArgsKargs {
                        args: vec![cls_arg, a, b],
                        kwargs: KwargsValues::Empty,
                    },
                    ArgValues::Kwargs(kw) => ArgValues::ArgsKargs {
                        args: vec![cls_arg],
                        kwargs: kw,
                    },
                    ArgValues::ArgsKargs { mut args, kwargs } => {
                        args.insert(0, cls_arg);
                        ArgValues::ArgsKargs { args, kwargs }
                    }
                };
                self.call_function(func, new_args)
            }
            DescriptorKind::Regular(value) => {
                // Regular function: call directly
                self.call_function(value, args)
            }
        }
    }

    /// Calls a method via super() MRO lookup.
    ///
    /// Looks up the method starting from the next class after `current_class_id`
    /// in the instance's MRO, then calls it with the instance as `self`.
    fn call_super_method_with_ids(
        &mut self,
        instance_id: HeapId,
        current_class_id: HeapId,
        method_name_id: StringId,
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        let method_name = self.interns.get_str(method_name_id);

        // Get the MRO to search. If instance_id is an Instance, use its class's MRO.
        // If instance_id is a ClassObject (super() inside __new__), use its own MRO.
        let instance_class_id = match self.heap.get(instance_id) {
            HeapData::Instance(inst) => inst.class_id(),
            HeapData::ClassObject(_) => instance_id, // super() in __new__: cls IS the class
            _ => {
                args.drop_with_heap(self.heap);
                return Err(ExcType::type_error("super(): __self__ is not an instance".to_string()));
            }
        };

        let mro = if let HeapData::ClassObject(cls) = self.heap.get(instance_class_id) {
            cls.mro().to_vec()
        } else {
            args.drop_with_heap(self.heap);
            return Err(ExcType::type_error("super(): class has no MRO".to_string()));
        };

        // Find current_class_id in MRO, start searching after it
        let start_idx = mro.iter().position(|&id| id == current_class_id).map_or(0, |i| i + 1);

        // Search for method in classes after current_class_id
        let mut method_value = None;
        for &class_id in &mro[start_idx..] {
            if let HeapData::ClassObject(cls) = self.heap.get(class_id)
                && let Some(value) = cls.namespace().get_by_str(method_name, self.heap, self.interns)
            {
                method_value = Some(value.clone_with_heap(self.heap));
                break;
            }
        }

        #[expect(clippy::manual_let_else)]
        let method_value = if let Some(v) = method_value {
            v
        } else {
            // Special case: super().__new__(cls) -- if __new__ is not found in the
            // remaining MRO, treat it as object.__new__(cls) which creates a bare instance.
            let new_id: StringId = StaticStrings::DunderNew.into();
            if method_name_id == new_id {
                // object.__new__(cls) semantics: create a bare instance of the given class.
                // The first arg is the class to instantiate.
                let target_class_id = match &args {
                    ArgValues::One(Value::Ref(id)) => Some(*id),
                    _ => None,
                };
                if let Some(cls_id) = target_class_id
                    && matches!(self.heap.get(cls_id), HeapData::ClassObject(_))
                {
                    args.drop_with_heap(self.heap);
                    let instance_value = self.allocate_instance_for_class(cls_id)?;
                    return Ok(CallResult::Push(instance_value));
                }
                args.drop_with_heap(self.heap);
                return Err(ExcType::type_error("object.__new__(X): X is not a type object"));
            }
            args.drop_with_heap(self.heap);
            return Err(ExcType::attribute_error("super", method_name));
        };

        // Prepend instance as self argument
        self.heap.inc_ref(instance_id);
        let self_arg = Value::Ref(instance_id);

        let new_args = match args {
            ArgValues::Empty => ArgValues::One(self_arg),
            ArgValues::One(a) => ArgValues::Two(self_arg, a),
            ArgValues::Two(a, b) => ArgValues::ArgsKargs {
                args: vec![self_arg, a, b],
                kwargs: KwargsValues::Empty,
            },
            ArgValues::Kwargs(kw) => ArgValues::ArgsKargs {
                args: vec![self_arg],
                kwargs: kw,
            },
            ArgValues::ArgsKargs { mut args, kwargs } => {
                args.insert(0, self_arg);
                ArgValues::ArgsKargs { args, kwargs }
            }
        };

        self.call_function(method_value, new_args)
    }

    // ========================================================================
    // Dunder Protocol Dispatch
    // ========================================================================

    /// Looks up a dunder method on an instance's TYPE (not the instance itself).
    ///
    /// This implements the Python semantic that dunder methods are looked up on the
    /// type, not the instance. For example, `type(x).__add__(x, y)` not `x.__add__(y)`.
    ///
    /// Returns `Some(method_value)` if found, `None` if not found.
    /// The returned value is cloned with proper refcount handling if it's a Ref.
    pub(super) fn lookup_type_dunder(&mut self, instance_heap_id: HeapId, dunder_name_id: StringId) -> Option<Value> {
        let dunder_name = self.interns.get_str(dunder_name_id);

        // Get the class_id from the instance
        let class_id = match self.heap.get(instance_heap_id) {
            HeapData::Instance(inst) => inst.class_id(),
            _ => return None,
        };

        // Look up in the class namespace via MRO (NOT instance attrs)
        match self.heap.get(class_id) {
            HeapData::ClassObject(cls) => cls
                .mro_lookup_attr(dunder_name, class_id, self.heap, self.interns)
                .map(|(v, _found_in)| v),
            _ => None,
        }
    }

    /// Looks up a dunder method on a class object's METACLASS.
    ///
    /// Used for metaclass hooks like `__getattribute__`, `__getattr__`,
    /// `__instancecheck__`, and `__subclasscheck__`.
    pub(super) fn lookup_metaclass_dunder(&mut self, class_heap_id: HeapId, dunder_name_id: StringId) -> Option<Value> {
        let dunder_name = self.interns.get_str(dunder_name_id);

        let HeapData::ClassObject(class_obj) = self.heap.get(class_heap_id) else {
            return None;
        };

        let metaclass_val = class_obj.metaclass();
        let meta_id = match metaclass_val {
            Value::Ref(id) => *id,
            _ => return None,
        };

        match self.heap.get(meta_id) {
            HeapData::ClassObject(meta_cls) => meta_cls
                .mro_lookup_attr(dunder_name, meta_id, self.heap, self.interns)
                .map(|(v, _)| v),
            _ => None,
        }
    }

    /// Calls a dunder method on an instance with given args.
    ///
    /// Prepends the instance as `self` argument, increments instance refcount.
    /// Returns `CallResult` which may be `FramePushed` for user-defined methods.
    pub(super) fn call_dunder(
        &mut self,
        instance_heap_id: HeapId,
        method_value: Value,
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        // Increment instance refcount for the self argument
        self.heap.inc_ref(instance_heap_id);
        let self_arg = Value::Ref(instance_heap_id);

        let new_args = match args {
            ArgValues::Empty => ArgValues::One(self_arg),
            ArgValues::One(a) => ArgValues::Two(self_arg, a),
            ArgValues::Two(a, b) => ArgValues::ArgsKargs {
                args: vec![self_arg, a, b],
                kwargs: KwargsValues::Empty,
            },
            ArgValues::Kwargs(kw) => ArgValues::ArgsKargs {
                args: vec![self_arg],
                kwargs: kw,
            },
            ArgValues::ArgsKargs { mut args, kwargs } => {
                args.insert(0, self_arg);
                ArgValues::ArgsKargs { args, kwargs }
            }
        };

        self.call_function(method_value, new_args)
    }

    /// Calls a dunder method on a class object, prepending the class as `self`.
    ///
    /// This is used for metaclass hooks like `__prepare__`, `__mro_entries__`,
    /// `__instancecheck__`, and `__subclasscheck__`, where the class object itself
    /// is the receiver.
    pub(super) fn call_class_dunder(
        &mut self,
        class_heap_id: HeapId,
        method_value: Value,
        args: ArgValues,
    ) -> Result<CallResult, RunError> {
        self.heap.inc_ref(class_heap_id);
        let cls_arg = Value::Ref(class_heap_id);
        let new_args = match args {
            ArgValues::Empty => ArgValues::One(cls_arg),
            ArgValues::One(a) => ArgValues::Two(cls_arg, a),
            ArgValues::Two(a, b) => ArgValues::ArgsKargs {
                args: vec![cls_arg, a, b],
                kwargs: KwargsValues::Empty,
            },
            ArgValues::Kwargs(kw) => ArgValues::ArgsKargs {
                args: vec![cls_arg],
                kwargs: kw,
            },
            ArgValues::ArgsKargs { mut args, kwargs } => {
                args.insert(0, cls_arg);
                ArgValues::ArgsKargs { args, kwargs }
            }
        };
        self.call_function(method_value, new_args)
    }

    /// Executes a binary dunder operation: tries `lhs.__op__(rhs)`, then `rhs.__rop__(lhs)`.
    ///
    /// Returns `Ok(Some(CallResult))` if a dunder was found and called,
    /// `Ok(None)` if neither operand has the dunder.
    ///
    /// Handles the NotImplemented protocol: if `__op__` returns NotImplemented,
    /// falls through to try `__rop__`.
    pub(super) fn try_binary_dunder(
        &mut self,
        lhs: &Value,
        rhs: &Value,
        dunder_id: StringId,
        reflected_dunder_id: Option<StringId>,
    ) -> Result<Option<CallResult>, RunError> {
        // Try lhs.__op__(rhs) - look up on TYPE, not instance
        if let Value::Ref(lhs_id) = lhs
            && matches!(self.heap.get(*lhs_id), HeapData::Instance(_))
            && let Some(method) = self.lookup_type_dunder(*lhs_id, dunder_id)
        {
            // Clone rhs for the call arg
            let rhs_clone = rhs.clone_with_heap(self.heap);
            let result = self.call_dunder(*lhs_id, method, ArgValues::One(rhs_clone))?;
            return Ok(Some(result));
        }

        // Try rhs.__rop__(lhs) if provided
        if let Some(ref_dunder_id) = reflected_dunder_id
            && let Value::Ref(rhs_id) = rhs
            && matches!(self.heap.get(*rhs_id), HeapData::Instance(_))
            && let Some(method) = self.lookup_type_dunder(*rhs_id, ref_dunder_id)
        {
            // Clone lhs for the call arg
            let lhs_clone = lhs.clone_with_heap(self.heap);
            let result = self.call_dunder(*rhs_id, method, ArgValues::One(lhs_clone))?;
            return Ok(Some(result));
        }

        Ok(None)
    }

    /// Executes an in-place dunder operation: tries `lhs.__iop__(rhs)`, falls back to `lhs.__op__(rhs)`.
    ///
    /// Returns `Ok(Some(CallResult))` if a dunder was found and called,
    /// `Ok(None)` if the instance has no relevant dunder.
    pub(super) fn try_inplace_dunder(
        &mut self,
        lhs: &Value,
        rhs: &Value,
        inplace_dunder_id: StringId,
        dunder_id: StringId,
        reflected_dunder_id: Option<StringId>,
    ) -> Result<Option<CallResult>, RunError> {
        // Try lhs.__iop__(rhs) first
        if let Value::Ref(lhs_id) = lhs
            && matches!(self.heap.get(*lhs_id), HeapData::Instance(_))
            && let Some(method) = self.lookup_type_dunder(*lhs_id, inplace_dunder_id)
        {
            let rhs_clone = rhs.clone_with_heap(self.heap);
            let result = self.call_dunder(*lhs_id, method, ArgValues::One(rhs_clone))?;
            return Ok(Some(result));
        }

        // Fall back to binary dunder
        self.try_binary_dunder(lhs, rhs, dunder_id, reflected_dunder_id)
    }

    /// Executes a unary dunder operation: tries `operand.__op__()`.
    ///
    /// Returns `Ok(Some(CallResult))` if the dunder was found and called,
    /// `Ok(None)` if the instance has no such dunder.
    pub(super) fn try_unary_dunder(
        &mut self,
        operand: &Value,
        dunder_id: StringId,
    ) -> Result<Option<CallResult>, RunError> {
        if let Value::Ref(id) = operand
            && matches!(self.heap.get(*id), HeapData::Instance(_))
            && let Some(method) = self.lookup_type_dunder(*id, dunder_id)
        {
            let result = self.call_dunder(*id, method, ArgValues::Empty)?;
            return Ok(Some(result));
        }
        Ok(None)
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
