use crate::{
    args::ArgValues,
    builtins::Builtins,
    evaluate::EvalResult,
    exception_private::{exc_fmt, ExcType},
    expressions::Identifier,
    heap::{Heap, HeapData},
    intern::Interns,
    io::PrintWriter,
    namespace::{NamespaceId, Namespaces},
    parse::CodeRange,
    resource::ResourceTracker,
    run_frame::RunResult,
    snapshot::{AbstractSnapshotTracker, ExternalCall},
    types::{PyTrait, Type},
    value::Value,
};

/// Target of a function call expression.
///
/// Represents a callable that can be either:
/// - A builtin function or exception resolved at parse time (`print`, `len`, `ValueError`, etc.)
/// - A name that will be looked up in the namespace at runtime (for callable variables)
///
/// Separate from Value to allow deriving Clone without Value's Clone restrictions.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum Callable {
    /// A builtin function like `print`, `len`, `str`, etc.
    Builtin(Builtins),
    /// A name to be looked up in the namespace at runtime (e.g., `x` in `x = len; x('abc')`).
    Name(Identifier),
}

impl Callable {
    /// Calls this callable with the given arguments.
    ///
    /// Returns `EvalResult::Value` for immediate results, or `EvalResult::ExternalCall`
    /// when execution is suspended waiting for an external function call.
    ///
    /// # Arguments
    /// * `namespaces` - The namespace namespaces containing all namespaces
    /// * `local_idx` - Index of the local namespace in namespaces
    /// * `heap` - The heap for allocating objects
    /// * `args` - The arguments to pass to the callable
    /// * `interns` - String storage for looking up interned names in error messages
    /// * `print` - The print for print output
    /// * `snapshot_tracker` - Tracker for recording execution position for resumption
    /// * `call_position` - Source position of the call expression for tracebacks
    #[allow(clippy::too_many_arguments)]
    pub fn call(
        &self,
        namespaces: &mut Namespaces,
        local_idx: NamespaceId,
        heap: &mut Heap<impl ResourceTracker>,
        args: ArgValues,
        interns: &Interns,
        print: &mut impl PrintWriter,
        snapshot_tracker: &mut impl AbstractSnapshotTracker,
        call_position: CodeRange,
    ) -> RunResult<EvalResult<Value>> {
        match self {
            Callable::Builtin(b) => b.call(heap, args, interns, print).map(EvalResult::Value),
            Callable::Name(ident) => {
                let mut args_opt = Some(args);
                // Look up the callable in the namespace
                let value = match namespaces.get_var(local_idx, ident, interns) {
                    Ok(value) => value,
                    Err(err) => {
                        if let Some(args) = args_opt.take() {
                            args.drop_with_heap(heap);
                        }
                        return Err(err);
                    }
                };

                match value {
                    Value::Builtin(builtin) => {
                        let args = args_opt.take().expect("args moved twice");
                        return builtin.call(heap, args, interns, print).map(EvalResult::Value);
                    }
                    Value::Function(f_id) => {
                        let f_id = *f_id;
                        // Check for cached return value (resuming after external call inside function).
                        // Use exact match to avoid consuming cached values from external function calls
                        // (which have None position) when this is a nested user-defined function call.
                        return match namespaces.take_ext_return_value_exact(heap, call_position) {
                            Ok(Some(return_value)) => {
                                // When resuming from an external call inside the function,
                                // the args were re-evaluated and need to be dropped
                                if let Some(args) = args_opt.take() {
                                    args.drop_with_heap(heap);
                                }
                                Ok(EvalResult::Value(return_value))
                            }
                            Ok(None) => {
                                // No cached return - call the function normally
                                let args = args_opt.take().expect("args moved twice");
                                interns.get_function(f_id).call(
                                    f_id,
                                    namespaces,
                                    heap,
                                    args,
                                    &[],
                                    interns,
                                    print,
                                    snapshot_tracker,
                                    call_position,
                                )
                            }
                            Err(e) => {
                                // External call inside function raised an exception
                                if let Some(args) = args_opt.take() {
                                    args.drop_with_heap(heap);
                                }
                                Err(e)
                            }
                        };
                    }
                    Value::ExtFunction(f_id) => {
                        let f_id = *f_id;
                        return match namespaces.take_ext_return_value(heap, call_position) {
                            Ok(Some(return_value)) => {
                                // When resuming from an external call, the args were re-evaluated
                                // and need to be dropped since we're using the cached return value
                                if let Some(args) = args_opt.take() {
                                    args.drop_with_heap(heap);
                                }
                                Ok(EvalResult::Value(return_value))
                            }
                            Ok(None) => {
                                // First call - make external function call
                                let args = args_opt
                                    .take()
                                    .expect("external function args already taken before making call");
                                Ok(EvalResult::ExternalCall(ExternalCall::new(f_id, args, call_position)))
                            }
                            Err(e) => {
                                // External function raised an exception - propagate it
                                if let Some(args) = args_opt.take() {
                                    args.drop_with_heap(heap);
                                }
                                Err(e)
                            }
                        };
                    }
                    // Check for heap-allocated closure or function with defaults
                    Value::Ref(heap_id) => {
                        let heap_id = *heap_id;
                        // Check for cached return value first (resuming after external call inside function).
                        // Use exact match to avoid consuming cached values from external function calls.
                        return match namespaces.take_ext_return_value_exact(heap, call_position) {
                            Ok(Some(return_value)) => {
                                if let Some(args) = args_opt.take() {
                                    args.drop_with_heap(heap);
                                }
                                Ok(EvalResult::Value(return_value))
                            }
                            Ok(None) => {
                                // No cached return - call the function normally
                                // Use with_entry_mut to temporarily take the HeapData out,
                                // allowing us to borrow heap mutably for the function call
                                let args = args_opt.take().expect("args moved twice");
                                heap.with_entry_mut(heap_id, |heap, data| match data {
                                    HeapData::Closure(f_id, cells, defaults) => {
                                        let f_id = *f_id;
                                        let f = interns.get_function(f_id);
                                        f.call_with_cells(
                                            f_id,
                                            namespaces,
                                            heap,
                                            args,
                                            cells,
                                            defaults,
                                            interns,
                                            print,
                                            snapshot_tracker,
                                            call_position,
                                        )
                                    }
                                    HeapData::FunctionDefaults(f_id, defaults) => {
                                        let f_id = *f_id;
                                        let f = interns.get_function(f_id);
                                        f.call(
                                            f_id,
                                            namespaces,
                                            heap,
                                            args,
                                            defaults,
                                            interns,
                                            print,
                                            snapshot_tracker,
                                            call_position,
                                        )
                                    }
                                    _ => {
                                        args.drop_with_heap(heap);
                                        // Not a callable heap type
                                        let type_name = data.py_type(Some(heap));
                                        let err = exc_fmt!(ExcType::TypeError; "'{type_name}' object is not callable");
                                        Err(err.with_position(ident.position).into())
                                    }
                                })
                            }
                            Err(e) => {
                                if let Some(args) = args_opt.take() {
                                    args.drop_with_heap(heap);
                                }
                                Err(e)
                            }
                        };
                    }
                    _ => {}
                }
                if let Some(args) = args_opt.take() {
                    args.drop_with_heap(heap);
                }
                let type_name = value.py_type(Some(heap));
                let err = exc_fmt!(ExcType::TypeError; "'{type_name}' object is not callable");
                Err(err.with_position(ident.position).into())
            }
        }
    }

    /// Returns true if this Callable is equal to another Callable.
    ///
    /// We assume functions with the same name and position in code are equal.
    pub fn py_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Builtin(b1), Self::Builtin(b2)) => b1 == b2,
            (Self::Name(n1), Self::Name(n2)) => n1.py_eq(n2),
            _ => false,
        }
    }

    pub fn py_type(&self) -> Type {
        match self {
            Self::Builtin(b) => b.py_type(),
            Self::Name(_) => Type::Function,
        }
    }

    /// Returns the callable name for error messages.
    ///
    /// For builtins, returns the builtin name (e.g., "print", "len") as a static str.
    /// For named callables, returns the function name from interns.
    pub fn name<'a>(&self, interns: &'a Interns) -> &'a str {
        match self {
            Self::Builtin(Builtins::Function(f)) => (*f).into(),
            Self::Builtin(Builtins::ExcType(e)) => (*e).into(),
            Self::Builtin(Builtins::Type(t)) => (*t).into(),
            Self::Name(ident) => interns.get_str(ident.name_id),
        }
    }
}
