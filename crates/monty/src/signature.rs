//! Function signature representation and argument binding.
//!
//! This module handles Python function signatures including all parameter types:
//! positional-only, positional-or-keyword, *args, keyword-only, and **kwargs.
//! It also handles default values and the argument binding algorithm.

use crate::{
    args::{ArgValues, KwargsValues},
    exception_private::{ExcType, SimpleException},
    expressions::Identifier,
    heap::{Heap, HeapData},
    intern::{Interns, StringId},
    resource::ResourceTracker,
    run_frame::RunResult,
    types::{Dict, Tuple},
    value::Value,
};

/// Represents a Python function signature with all parameter types.
///
/// A complete Python signature can include:
/// - Positional-only parameters (before `/`)
/// - Positional-or-keyword parameters (regular parameters)
/// - Variable positional parameter (`*args`)
/// - Keyword-only parameters (after `*` or `*args`)
/// - Variable keyword parameter (`**kwargs`)
///
/// # Default Values
///
/// Default values are tracked by count per parameter group. The `*_defaults_count` fields
/// indicate how many parameters (from the end of each group) have defaults. For example,
/// if `args = [a, b, c]` and `arg_defaults_count = 2`, then `b` and `c` have defaults.
///
/// Note: The actual default Values are evaluated at function definition time and stored
/// separately (in the heap as part of the function/closure object). This struct only
/// tracks the structure, not the values themselves.
///
/// # Namespace Layout
///
/// Parameters are laid out in the namespace in this order:
/// ```text
/// [pos_args][args][*args_slot?][kwargs][**kwargs_slot?]
/// ```
/// The `*args` slot is only present if `var_args` is Some.
/// The `**kwargs` slot is only present if `var_kwargs` is Some.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Signature {
    /// Positional-only parameters, e.g. `a, b` in `def f(a, b, /): ...`
    ///
    /// These can only be passed by position, not by keyword.
    pub pos_args: Option<Vec<StringId>>,

    /// Number of positional-only parameters with defaults (from the end).
    pub pos_defaults_count: usize,

    /// Positional-or-keyword parameters, e.g. `a, b` in `def f(a, b): ...`
    ///
    /// These can be passed either by position or by keyword.
    pub args: Option<Vec<StringId>>,

    /// Number of positional-or-keyword parameters with defaults (from the end).
    pub arg_defaults_count: usize,

    /// Variable positional parameter name, e.g. `args` in `def f(*args): ...`
    ///
    /// Collects excess positional arguments into a tuple.
    pub var_args: Option<StringId>,

    /// Keyword-only parameters, e.g. `c` in `def f(*, c): ...` or `def f(*args, c): ...`
    ///
    /// These can only be passed by keyword, not by position.
    pub kwargs: Option<Vec<StringId>>,

    /// Mapping from each keyword-only parameter to its default index (if any).
    ///
    /// Each entry corresponds to the same index in `kwargs`. A value of `Some(i)`
    /// points into the kwarg section of the defaults array, while `None` means
    /// the parameter is required.
    pub kwarg_default_map: Option<Vec<Option<usize>>>,

    /// Variable keyword parameter name, e.g. `kwargs` in `def f(**kwargs): ...`
    ///
    /// Collects excess keyword arguments into a dict.
    pub var_kwargs: Option<StringId>,
}

impl Signature {
    /// Creates a full signature with all parameter types.
    ///
    /// # Arguments
    /// * `pos_args` - Positional-only parameter names
    /// * `pos_defaults_count` - Number of pos_args with defaults (from end)
    /// * `args` - Positional-or-keyword parameter names
    /// * `arg_defaults_count` - Number of args with defaults (from end)
    /// * `var_args` - Variable positional parameter name (*args)
    /// * `kwargs` - Keyword-only parameter names
    /// * `kwarg_default_map` - Mapping of kw-only parameters to default indices
    /// * `var_kwargs` - Variable keyword parameter name (**kwargs)
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pos_args: Vec<StringId>,
        pos_defaults_count: usize,
        args: Vec<StringId>,
        arg_defaults_count: usize,
        var_args: Option<StringId>,
        kwargs: Vec<StringId>,
        kwarg_default_map: Vec<Option<usize>>,
        var_kwargs: Option<StringId>,
    ) -> Self {
        let has_kwonly = !kwargs.is_empty();
        Self {
            pos_args: if pos_args.is_empty() { None } else { Some(pos_args) },
            pos_defaults_count,
            args: if args.is_empty() { None } else { Some(args) },
            arg_defaults_count,
            var_args,
            kwargs: if has_kwonly { Some(kwargs) } else { None },
            kwarg_default_map: if has_kwonly { Some(kwarg_default_map) } else { None },
            var_kwargs,
        }
    }

    /// Returns true if this is a simple signature (no defaults, no *args/**kwargs).
    ///
    /// Simple signatures can use a fast path for argument binding that avoids
    /// the full binding algorithm overhead. A simple signature has:
    /// - No positional-only parameters
    /// - No defaults for any parameters
    /// - No *args or **kwargs
    /// - No keyword-only parameters
    pub fn is_simple(&self) -> bool {
        self.pos_args.is_none()
            && self.pos_defaults_count == 0
            && self.arg_defaults_count == 0
            && self.var_args.is_none()
            && self.kwargs.is_none()
            && self.var_kwargs.is_none()
    }

    /// Returns the total number of default values across all parameter groups.
    pub fn total_defaults_count(&self) -> usize {
        self.pos_defaults_count + self.arg_defaults_count + self.kwarg_defaults_count()
    }

    fn kwarg_defaults_count(&self) -> usize {
        self.kwarg_default_map
            .as_deref()
            .map(|v| v.iter().filter(|&x| x.is_some()).count())
            .unwrap_or_default()
    }

    /// Returns the number of positional-only parameters.
    pub fn pos_arg_count(&self) -> usize {
        self.pos_args.as_ref().map_or(0, Vec::len)
    }

    /// Returns the number of positional-or-keyword parameters.
    pub fn arg_count(&self) -> usize {
        self.args.as_ref().map_or(0, Vec::len)
    }

    /// Returns the number of keyword-only parameters.
    pub fn kwarg_count(&self) -> usize {
        self.kwargs.as_ref().map_or(0, Vec::len)
    }

    /// Returns the total number of named parameters (excluding *args/**kwargs slots).
    ///
    /// This is `pos_args.len() + args.len() + kwargs.len()`.
    pub fn param_count(&self) -> usize {
        self.pos_arg_count() + self.arg_count() + self.kwarg_count()
    }

    /// Returns the total number of namespace slots needed for parameters.
    ///
    /// This includes slots for:
    /// - All named parameters (pos_args + args + kwargs)
    /// - The *args tuple (if var_args is Some)
    /// - The **kwargs dict (if var_kwargs is Some)
    pub fn total_slots(&self) -> usize {
        let mut slots = self.param_count();
        if self.var_args.is_some() {
            slots += 1;
        }
        if self.var_kwargs.is_some() {
            slots += 1;
        }
        slots
    }

    /// Returns the namespace slot index for the *args tuple, if present.
    ///
    /// The *args slot comes after pos_args and args.
    pub fn var_args_slot(&self) -> Option<usize> {
        self.var_args.as_ref().map(|_| self.pos_arg_count() + self.arg_count())
    }

    /// Returns the namespace slot index for the **kwargs dict, if present.
    ///
    /// The **kwargs slot comes after all other parameters including kwargs.
    pub fn var_kwargs_slot(&self) -> Option<usize> {
        self.var_kwargs.as_ref().map(|_| {
            let mut slot = self.pos_arg_count() + self.arg_count() + self.kwarg_count();
            if self.var_args.is_some() {
                slot += 1;
            }
            slot
        })
    }

    /// Returns an iterator over all parameter names in namespace slot order.
    ///
    /// Order: pos_args, args, var_args (if present), kwargs, var_kwargs (if present)
    pub fn param_names(&self) -> impl Iterator<Item = StringId> + '_ {
        let pos_args = self.pos_args.iter().flat_map(|v| v.iter().copied());
        let args = self.args.iter().flat_map(|v| v.iter().copied());
        let var_args = self.var_args.iter().copied();
        let kwargs = self.kwargs.iter().flat_map(|v| v.iter().copied());
        let var_kwargs = self.var_kwargs.iter().copied();

        pos_args.chain(args).chain(var_args).chain(kwargs).chain(var_kwargs)
    }

    /// Returns the number of required positional arguments.
    ///
    /// This is the minimum number of positional arguments that must be provided.
    /// Parameters with defaults are not required.
    pub fn required_positional_count(&self) -> usize {
        let total_positional = self.pos_arg_count() + self.arg_count();
        let positional_defaults = self.pos_defaults_count + self.arg_defaults_count;
        total_positional.saturating_sub(positional_defaults)
    }

    /// Returns the number of required keyword-only arguments.
    ///
    /// This is the minimum number of keyword-only arguments that must be provided.
    /// Keyword-only parameters with defaults are not required.
    pub fn required_kwonly_count(&self) -> usize {
        self.kwarg_count().saturating_sub(self.kwarg_defaults_count())
    }

    /// Returns the maximum number of positional arguments accepted.
    ///
    /// Returns None if *args is present (unlimited positional args).
    pub fn max_positional_count(&self) -> Option<usize> {
        if self.var_args.is_some() {
            None
        } else {
            Some(self.pos_arg_count() + self.arg_count())
        }
    }

    /// Binds arguments to parameters according to Python's calling conventions.
    ///
    /// This implements the full argument binding algorithm:
    /// 1. Bind positional args to pos_args, then args (in order)
    /// 2. Bind keyword args to args and kwargs (NOT pos_args - positional-only)
    /// 3. Collect excess positional args into *args tuple
    /// 4. Collect excess keyword args into **kwargs dict
    /// 5. Apply defaults for missing parameters
    ///
    /// Returns a Vec<Value> ready to be injected into the namespace, laid out as:
    /// `[pos_args][args][*args_slot?][kwargs][**kwargs_slot?]`
    ///
    /// # Arguments
    /// * `args` - The arguments from the call site
    /// * `defaults` - Evaluated default values (layout: pos_defaults, arg_defaults, kwarg_defaults)
    /// * `heap` - The heap for allocating *args tuple and **kwargs dict
    /// * `interns` - For looking up parameter names in error messages
    /// * `func_name` - Function name for error messages
    /// * `namespace_size` - The size of the namespace to allocate
    ///
    /// # Errors
    /// Returns an error if:
    /// - Too few or too many positional arguments
    /// - Missing required keyword-only arguments
    /// - Unexpected keyword argument
    /// - Positional-only parameter passed as keyword
    /// - Same argument passed both positionally and by keyword
    pub fn bind(
        &self,
        mut args: ArgValues,
        defaults: &[Value],
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
        func_name: Identifier,
        namespace: &mut Vec<Value>,
    ) -> RunResult<()> {
        if self.is_simple() {
            // Injects arguments into a namespace for simple function signatures.
            //
            // This is the fast path for functions with only positional parameters and no
            // defaults, *args, kwargs or **kwargs.
            //
            // We do a weird thing here where we return Option<ArgsValues> so we can consume args, but
            // still use them again in the non-simple case.
            let opt_args = match args {
                ArgValues::Empty => None,
                ArgValues::One(a) => {
                    namespace.push(a);
                    None
                }
                ArgValues::Two(a1, a2) => {
                    namespace.push(a1);
                    namespace.push(a2);
                    None
                }
                ArgValues::ArgsKargs {
                    args,
                    kwargs: KwargsValues::Empty,
                } => {
                    namespace.extend(args);
                    None
                }
                args => Some(args),
            };
            if let Some(continue_args) = opt_args {
                args = continue_args;
            } else {
                let actual_count = namespace.len();
                return if actual_count == self.param_count() {
                    Ok(())
                } else {
                    // Clean up bound values before returning error
                    for val in namespace.drain(..) {
                        val.drop_with_heap(heap);
                    }
                    self.wrong_arg_count_error(actual_count, interns, func_name)
                };
            }
        }
        // Full binding algorithm for complex signatures or kwargs

        // Split args into positional and keyword components
        let (positional_args, keyword_args) = args.split();

        // Calculate how many positional params we have
        let pos_param_count = self.pos_arg_count();
        let arg_param_count = self.arg_count();
        let total_positional_params = pos_param_count + arg_param_count;

        // Check positional argument count against maximum
        let positional_count = positional_args.len();
        let kwonly_given = keyword_args.len();
        if let Some(max) = self.max_positional_count() {
            if positional_count > max {
                let func = interns.get_str(func_name.name_id);
                return Err(ExcType::type_error_too_many_positional(
                    func,
                    max,
                    positional_count,
                    kwonly_given,
                ));
            }
        }

        // Initialize result namespace with Undefined values for all slots
        // Layout: [pos_args][args][*args?][kwargs][**kwargs?]
        let var_args_offset = usize::from(self.var_args.is_some());
        let all_named_slots = total_positional_params + self.kwarg_count();
        for _ in 0..self.total_slots() {
            namespace.push(Value::Undefined);
        }

        // Track which parameters have been bound (for duplicate detection)
        // Note: this tracks only named params, not *args/**kwargs slots
        let mut bound_params = vec![false; all_named_slots];

        // 1. Bind positional args to pos_args, then args
        let mut pos_iter = positional_args.into_iter();

        // Bind to pos_args
        for i in 0..pos_param_count {
            if let Some(val) = pos_iter.next() {
                namespace[i] = val;
                bound_params[i] = true;
            }
        }

        // Bind to args
        for i in pos_param_count..total_positional_params {
            if let Some(val) = pos_iter.next() {
                namespace[i] = val;
                bound_params[i] = true;
            }
        }

        // 2. Collect excess positional args into *args tuple
        let excess_positional: Vec<Value> = pos_iter.collect();
        let var_args_value = if self.var_args.is_some() {
            // Create tuple from excess args
            let tuple_id = heap.allocate(HeapData::Tuple(Tuple::new(excess_positional)))?;
            Some(Value::Ref(tuple_id))
        } else {
            None
        };
        // If no *args, excess was already checked above via max_positional_count

        // 3. Bind keyword args
        // Bind keywords to args and kwargs (not pos_args - those are positional-only)
        let mut excess_kwargs = Dict::new();

        for (key, value) in keyword_args {
            let Some(keyword_name) = key.as_either_str(heap) else {
                key.drop_with_heap(heap);
                value.drop_with_heap(heap);
                cleanup_on_error(namespace, var_args_value, excess_kwargs, heap);
                return Err(ExcType::type_error("keywords must be strings"));
            };

            // Check if this keyword matches a positional-only param (error)
            if let Some(pos_args) = &self.pos_args {
                if let Some(&param_id) = pos_args
                    .iter()
                    .find(|&&param_id| keyword_name.matches(param_id, interns))
                {
                    let func = interns.get_str(func_name.name_id);
                    let param = interns.get_str(param_id);
                    key.drop_with_heap(heap);
                    value.drop_with_heap(heap);
                    cleanup_on_error(namespace, var_args_value, excess_kwargs, heap);
                    return Err(ExcType::type_error_positional_only(func, param));
                }
            }

            // Use Option to track the value as we try to bind it
            let mut remaining_value = Some(value);
            let mut key_value = Some(key);

            // Try to bind to an args param
            if let Some(ref args) = self.args {
                let matching_param = args
                    .iter()
                    .enumerate()
                    .find(|&(_, param_id)| keyword_name.matches(*param_id, interns));
                if let Some((i, &param_id)) = matching_param {
                    let idx = pos_param_count + i;
                    if bound_params[idx] {
                        let func = interns.get_str(func_name.name_id);
                        let param = interns.get_str(param_id);
                        if let Some(v) = remaining_value.take() {
                            v.drop_with_heap(heap);
                        }
                        if let Some(dup_key) = key_value.take() {
                            dup_key.drop_with_heap(heap);
                        }
                        cleanup_on_error(namespace, var_args_value, excess_kwargs, heap);
                        return Err(ExcType::type_error_duplicate_arg(func, param));
                    }
                    if let Some(v) = remaining_value.take() {
                        namespace[idx] = v;
                    }
                    bound_params[idx] = true;
                    if let Some(key) = key_value.take() {
                        key.drop_with_heap(heap);
                    }
                }
            }

            // Try to bind to a kwargs param (keyword-only)
            if remaining_value.is_some() {
                if let Some(ref kwargs) = self.kwargs {
                    for (i, &param_id) in kwargs.iter().enumerate() {
                        if keyword_name.matches(param_id, interns) {
                            // Skip past *args slot if present
                            let ns_idx = total_positional_params + var_args_offset + i;
                            let idx = total_positional_params + i;
                            if bound_params[idx] {
                                let func = interns.get_str(func_name.name_id);
                                let param = interns.get_str(param_id);
                                if let Some(v) = remaining_value.take() {
                                    v.drop_with_heap(heap);
                                }
                                if let Some(dup_key) = key_value.take() {
                                    dup_key.drop_with_heap(heap);
                                }
                                cleanup_on_error(namespace, var_args_value, excess_kwargs, heap);
                                return Err(ExcType::type_error_duplicate_arg(func, param));
                            }
                            // Store the value for this keyword-only param
                            if let Some(v) = remaining_value.take() {
                                namespace[ns_idx] = v;
                            }
                            bound_params[idx] = true;
                            if let Some(bound_key) = key_value.take() {
                                bound_key.drop_with_heap(heap);
                            }
                            break;
                        }
                    }
                }
            }

            // If still not bound, handle as excess or error
            if let Some(v) = remaining_value {
                if self.var_kwargs.is_some() {
                    // Collect into **kwargs
                    let key_for_kwargs = key_value.take().expect("keyword key available for **kwargs");
                    excess_kwargs.set(key_for_kwargs, v, heap, interns)?;
                } else {
                    let func = interns.get_str(func_name.name_id);
                    let key_str = keyword_name.as_str(interns);
                    v.drop_with_heap(heap);
                    if let Some(unused_key) = key_value.take() {
                        unused_key.drop_with_heap(heap);
                    }
                    cleanup_on_error(namespace, var_args_value, excess_kwargs, heap);
                    return Err(ExcType::type_error_unexpected_keyword(func, key_str));
                }
            }

            if let Some(unused_key) = key_value {
                unused_key.drop_with_heap(heap);
            }
        }

        // 3.5. Apply default values to unbound optional parameters
        // Defaults layout: [pos_defaults...][arg_defaults...][kwarg_defaults...]
        // Each section only contains defaults for params that have them.
        let mut default_idx = 0;

        // Apply pos_args defaults (optional params at the end of pos_args)
        if self.pos_defaults_count > 0 {
            let first_optional = pos_param_count - self.pos_defaults_count;
            for i in first_optional..pos_param_count {
                if !bound_params[i] {
                    namespace[i] = defaults[default_idx + (i - first_optional)].clone_with_heap(heap);
                    bound_params[i] = true;
                }
            }
        }
        default_idx += self.pos_defaults_count;

        // Apply args defaults (optional params at the end of args)
        if self.arg_defaults_count > 0 {
            let first_optional = arg_param_count - self.arg_defaults_count;
            for i in first_optional..arg_param_count {
                let ns_idx = pos_param_count + i;
                if !bound_params[ns_idx] {
                    namespace[ns_idx] = defaults[default_idx + (i - first_optional)].clone_with_heap(heap);
                    bound_params[ns_idx] = true;
                }
            }
        }
        default_idx += self.arg_defaults_count;

        // Apply kwargs defaults using the explicit default map
        if let Some(ref default_map) = self.kwarg_default_map {
            for (i, default_slot) in default_map.iter().enumerate() {
                if let Some(slot_idx) = default_slot {
                    let bound_idx = total_positional_params + i;
                    // Skip past *args slot if present
                    let ns_idx = total_positional_params + var_args_offset + i;
                    if !bound_params[bound_idx] {
                        namespace[ns_idx] = defaults[default_idx + slot_idx].clone_with_heap(heap);
                        bound_params[bound_idx] = true;
                    }
                }
            }
        }

        // 4. Check that all required params are bound BEFORE building final namespace.
        // This ensures we can clean up properly on error without leaking heap values.
        let func = interns.get_str(func_name.name_id);

        // Check required positional params (pos_args + required args)
        let mut missing_positional: Vec<&str> = Vec::new();

        // Check pos_args
        if let Some(ref pos_args) = self.pos_args {
            let required_pos_only = pos_args.len().saturating_sub(self.pos_defaults_count);
            for (i, &param_id) in pos_args.iter().enumerate() {
                if i < required_pos_only && !bound_params[i] {
                    missing_positional.push(interns.get_str(param_id));
                }
            }
        }

        // Check args (positional-or-keyword)
        if let Some(ref args_params) = self.args {
            let required_args = args_params.len().saturating_sub(self.arg_defaults_count);
            for (i, &param_id) in args_params.iter().enumerate() {
                if i < required_args && !bound_params[pos_param_count + i] {
                    missing_positional.push(interns.get_str(param_id));
                }
            }
        }

        if !missing_positional.is_empty() {
            // Clean up bound values before returning error
            cleanup_on_error(namespace, var_args_value, excess_kwargs, heap);
            return Err(ExcType::type_error_missing_positional_with_names(
                func,
                &missing_positional,
            ));
        }

        // Check required keyword-only args
        let mut missing_kwonly: Vec<&str> = Vec::new();
        if let Some(ref kwargs_params) = self.kwargs {
            let default_map = self.kwarg_default_map.as_ref();
            for (i, &param_id) in kwargs_params.iter().enumerate() {
                let has_default = default_map.and_then(|map| map.get(i)).is_some_and(Option::is_some);
                if !has_default && !bound_params[total_positional_params + i] {
                    missing_kwonly.push(interns.get_str(param_id));
                }
            }
        }

        if !missing_kwonly.is_empty() {
            // Clean up bound values before returning error
            cleanup_on_error(namespace, var_args_value, excess_kwargs, heap);
            return Err(ExcType::type_error_missing_kwonly_with_names(func, &missing_kwonly));
        }

        // 5. Fill in *args and **kwargs slots directly
        // Namespace layout: [pos_args][args][*args?][kwargs][**kwargs?]

        // Insert *args tuple if present
        if let Some(var_args_val) = var_args_value {
            namespace[total_positional_params] = var_args_val;
        }

        // Insert **kwargs dict if present (at the last slot)
        if self.var_kwargs.is_some() {
            let dict_id = heap.allocate(HeapData::Dict(excess_kwargs))?;
            let last_slot = namespace.len() - 1;
            namespace[last_slot] = Value::Ref(dict_id);
        }

        Ok(())
    }

    /// Creates an error for wrong number of arguments.
    ///
    /// Handles both "missing required positional arguments" and "too many arguments" cases,
    /// formatting the error message to match CPython's style.
    ///
    /// # Arguments
    /// * `actual_count` - Number of arguments actually provided
    /// * `interns` - String storage for looking up interned names
    fn wrong_arg_count_error<T>(&self, actual_count: usize, interns: &Interns, func_name: Identifier) -> RunResult<T> {
        let name_str = interns.get_str(func_name.name_id);
        let param_count = self.param_count();
        let msg = if let Some(missing_count) = param_count.checked_sub(actual_count) {
            // Missing arguments - show actual parameter names
            let mut msg = format!(
                "{}() missing {} required positional argument{}: ",
                name_str,
                missing_count,
                if missing_count == 1 { "" } else { "s" }
            );
            // Collect parameter names, skipping the ones already provided
            let mut missing_names: Vec<_> = self
                .param_names()
                .skip(actual_count)
                .map(|string_id| format!("'{}'", interns.get_str(string_id)))
                .collect();
            let last = missing_names.pop().unwrap();
            if !missing_names.is_empty() {
                msg.push_str(&missing_names.join(", "));
                msg.push_str(", and ");
            }
            msg.push_str(&last);
            msg
        } else {
            // Too many arguments
            format!(
                "{}() takes {} positional argument{} but {} {} given",
                name_str,
                param_count,
                if param_count == 1 { "" } else { "s" },
                actual_count,
                if actual_count == 1 { "was" } else { "were" }
            )
        };
        Err(SimpleException::new(ExcType::TypeError, Some(msg))
            .with_position(func_name.position)
            .into())
    }
}

/// Cleans up bound values when returning an error from `bind()`.
///
/// This function properly decrements reference counts for all heap-allocated
/// values that were bound during argument processing but need to be discarded
/// due to an error (e.g., missing required argument).
fn cleanup_on_error(
    namespace: &mut [Value],
    var_args_value: Option<Value>,
    excess_kwargs: Dict,
    heap: &mut Heap<impl ResourceTracker>,
) {
    // Clean up values in namespace
    for slot in namespace.iter_mut() {
        let value = std::mem::replace(slot, Value::Undefined);
        value.drop_with_heap(heap);
    }
    // Clean up *args tuple if allocated
    if let Some(val) = var_args_value {
        val.drop_with_heap(heap);
    }
    // Clean up excess kwargs dict contents (keys and values)
    for (key, value) in excess_kwargs {
        key.drop_with_heap(heap);
        value.drop_with_heap(heap);
    }
}
