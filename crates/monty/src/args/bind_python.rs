//! Signature representation and argument binding for user-defined *Python*
//! functions (`def` / `lambda`).
//!
//! This module handles Python function signatures including all parameter types:
//! positional-only, positional-or-keyword, *args, keyword-only, and **kwargs.
//! It also handles default values and the argument binding algorithm.
//!
//! Native (Rust-implemented) callables are bound by the sibling
//! [`bind_native`](super::bind_native) module instead. The two binders are
//! deliberately separate (different param representations, outputs, and
//! error families), but [`Signature::bind`] MUST stay behaviourally
//! identical to `bind_native`'s `ErrorFamily::Def` path — same
//! kwargs-before-overflow ordering, same wording, same `(and N keyword-only
//! argument(s))` counting. When changing `def` semantics in either file,
//! change both; coverage lives in `test_cases/function__arity_defaults.py`
//! and `test_cases/args__macro_errors.py`.

use std::cell::OnceCell;

use crate::{
    args::{ArgPosIter, ArgValues},
    bytecode::VM,
    defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    expressions::Identifier,
    heap::{DropGuard, DropWithContext, HeapData},
    intern::{Interns, StringId},
    types::{Dict, allocate_tuple},
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
pub(crate) struct Signature {
    /// Positional-only parameters, e.g. `a, b` in `def f(a, b, /): ...`
    ///
    /// These can only be passed by position, not by keyword.
    pos_args: Option<Vec<StringId>>,

    /// Number of positional-only parameters with defaults (from the end).
    pos_defaults_count: usize,

    /// Positional-or-keyword parameters, e.g. `a, b` in `def f(a, b): ...`
    ///
    /// These can be passed either by position or by keyword.
    args: Option<Vec<StringId>>,

    /// Number of positional-or-keyword parameters with defaults (from the end).
    arg_defaults_count: usize,

    /// Variable positional parameter name, e.g. `args` in `def f(*args): ...`
    ///
    /// Collects excess positional arguments into a tuple.
    var_args: Option<StringId>,

    /// Keyword-only parameters, e.g. `c` in `def f(*, c): ...` or `def f(*args, c): ...`
    ///
    /// These can only be passed by keyword, not by position.
    kwargs: Option<Vec<StringId>>,

    /// Mapping from each keyword-only parameter to its default index (if any).
    ///
    /// Each entry corresponds to the same index in `kwargs`. A value of `Some(i)`
    /// points into the kwarg section of the defaults array, while `None` means
    /// the parameter is required.
    kwarg_default_map: Option<Vec<Option<usize>>>,

    /// Variable keyword parameter name, e.g. `kwargs` in `def f(**kwargs): ...`
    ///
    /// Collects excess keyword arguments into a dict.
    var_kwargs: Option<StringId>,

    /// How simple the signature is, used for the fast path when binding.
    ///
    /// Derived from the other fields and cached lazily via [`Self::bind_mode`]
    /// rather than serialized: never storing a computed value on the wire
    /// means a signature loaded from an older REPL dump can't ever carry a
    /// `BindMode` stale with respect to the derivation logic in the binary
    /// that loads it.
    #[serde(skip)]
    bind_mode: OnceCell<BindMode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BindMode {
    /// If this is a simple signature (no defaults, no *args/**kwargs).
    ///
    /// Simple signatures can use a fast path for argument binding that avoids
    /// the full binding algorithm overhead. A simple signature has:
    /// - No positional-only parameters
    /// - No defaults for any parameters
    /// - No *args or **kwargs
    /// - No keyword-only parameters
    Simple,
    /// If this signature has only positional-or-keyword params with defaults.
    ///
    /// This identifies the common pattern `def f(a, b=1, c=2)` where:
    /// - No positional-only parameters
    /// - No *args or **kwargs
    /// - No keyword-only parameters
    /// - Has some default values
    ///
    /// These signatures can use a simplified binding that just fills positional
    /// args and applies defaults without the full algorithm overhead.
    SimpleWithDefaults,
    Complex,
}

/// Malformed signature metadata for dump tests.
#[cfg(feature = "test-hooks")]
#[derive(Debug, Clone, Copy)]
pub(crate) enum SignatureMetadataFault {
    /// Makes positional-only defaults outnumber their parameters.
    PosDefaultsCountOutOfRange,
    /// Makes positional defaults outnumber their parameters.
    ArgDefaultsCountOutOfRange,
    /// Breaks the keyword-only parameter/default-map pairing.
    KwargDefaultMapLengthMismatch,
    /// Makes keyword-only default indices non-contiguous.
    KwargDefaultIndexGap,
}

impl Signature {
    /// Returns the parameter count when positional arguments need no binding.
    ///
    /// With no defaults, variadic slots, or keyword-only parameters, positional
    /// arguments already have the namespace layout produced by [`Self::bind`].
    pub(crate) fn exact_positional_count(&self) -> Option<usize> {
        (self.pos_defaults_count == 0
            && self.arg_defaults_count == 0
            && self.var_args.is_none()
            && self.kwargs.is_none()
            && self.var_kwargs.is_none())
        .then(|| self.param_count())
    }

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
    #[expect(clippy::too_many_arguments)]
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
        let pos_args = if pos_args.is_empty() { None } else { Some(pos_args) };
        let has_kwonly = !kwargs.is_empty();
        let kwargs = if has_kwonly { Some(kwargs) } else { None };

        Self {
            pos_args,
            pos_defaults_count,
            args: if args.is_empty() { None } else { Some(args) },
            arg_defaults_count,
            var_args,
            kwargs,
            kwarg_default_map: if has_kwonly { Some(kwarg_default_map) } else { None },
            var_kwargs,
            bind_mode: OnceCell::new(),
        }
    }

    /// Returns the signature's [`BindMode`], deriving and caching it on first use.
    fn bind_mode(&self) -> BindMode {
        *self.bind_mode.get_or_init(|| self.derive_bind_mode())
    }

    /// Derives [`BindMode`] from the signature's parameter fields.
    fn derive_bind_mode(&self) -> BindMode {
        if self.pos_args.is_none()
            && self.pos_defaults_count == 0
            && self.arg_defaults_count == 0
            && self.var_args.is_none()
            && self.kwargs.is_none()
            && self.var_kwargs.is_none()
        {
            BindMode::Simple
        } else if self.pos_args.is_none()
            && self.var_args.is_none()
            && self.kwargs.is_none()
            && self.var_kwargs.is_none()
            && self.arg_defaults_count > 0
        {
            BindMode::SimpleWithDefaults
        } else {
            BindMode::Complex
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
    /// * `namespace` - The namespace to populate with bound arguments. This is mutated in place and will need to be cleaned up on error.
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
        args: ArgValues,
        defaults: &[Value],
        vm: &mut VM<'_>,
        func_name: Identifier,
        namespace: &mut Vec<Value>,
    ) -> RunResult<()> {
        let (pos_iter, keyword_args) = args.into_parts();
        let n_kwargs = keyword_args.len();
        let namespace_base = namespace.len();

        // Fast path for simple signatures (no special params) and signatures
        // with only positional-or-keyword params and defaults, when no keyword
        // arguments are passed. Handled before building the kwargs iterator +
        // guard below, which are pure overhead in this common positional case.
        if n_kwargs == 0 && matches!(self.bind_mode(), BindMode::Simple | BindMode::SimpleWithDefaults) {
            // No keyword arguments to bind; the empty container holds no refs.
            keyword_args.drop_with(vm);
            match pos_iter {
                ArgPosIter::Empty => {}
                ArgPosIter::One(a) => {
                    namespace.push(a);
                }
                ArgPosIter::Two([a1, a2]) => {
                    namespace.push(a1);
                    namespace.push(a2);
                }
                ArgPosIter::Vec(args) => {
                    namespace.extend(args);
                }
            }

            let actual_count = namespace.len() - namespace_base;
            let param_count = self.param_count();

            if actual_count == param_count {
                // Exact match - no defaults needed
                return Ok(());
            } else if self.bind_mode() == BindMode::SimpleWithDefaults {
                let required = self.required_positional_count();
                if actual_count >= required && actual_count < param_count {
                    // Apply defaults for remaining parameters
                    // Defaults are stored at the end of the defaults array for pos-or-kw params
                    let defaults_needed = param_count - actual_count;
                    let defaults_start = self.arg_defaults_count - defaults_needed;
                    for i in 0..defaults_needed {
                        namespace.push(defaults[defaults_start + i].clone_with_heap(vm));
                    }
                    return Ok(());
                }
            }

            return self.wrong_arg_count_error(actual_count, vm.interns, func_name);
        }

        // Full binding algorithm for complex signatures or kwargs. Convert
        // kwargs to an iterator and guard it so remaining items are cleaned up
        // on any error path.
        let keyword_args = keyword_args.into_iter();
        defer_drop_mut!(keyword_args, vm);

        // Keep unconsumed positional values guarded throughout binding.
        defer_drop_mut!(pos_iter, vm);

        // Calculate how many positional params we have
        let pos_param_count = self.pos_arg_count();
        let arg_param_count = self.arg_count();
        let total_positional_params = pos_param_count + arg_param_count;

        // Too many positionals? Noted here but raised only *after* the keyword
        // loop: CPython binds keyword arguments first, so unexpected-kwarg and
        // multiple-values errors beat too-many-positional.
        let positional_count = pos_iter.len();
        let positional_overflow = self.max_positional_count().is_some_and(|max| positional_count > max);

        // `Undefined` is internal-only, so each namespace slot also records whether
        // its parameter was bound. Layout: [pos_args][args][*args?][kwargs][**kwargs?]
        let var_args_offset = usize::from(self.var_args.is_some());
        namespace.resize_with(namespace.len() + self.total_slots(), || Value::Undefined);

        // 1. Bind positional args to pos_args, then args

        // Bind to pos_args
        for slot in namespace[namespace_base..].iter_mut().take(pos_param_count) {
            if let Some(val) = pos_iter.next() {
                *slot = val;
            }
        }

        // Bind to args
        for slot in namespace[namespace_base..]
            .iter_mut()
            .take(total_positional_params)
            .skip(pos_param_count)
        {
            if let Some(val) = pos_iter.next() {
                *slot = val;
            }
        }

        // 2. Collect excess positional args into *args tuple. Without `*args`
        // any excess (the deferred overflow) stays in `pos_iter`, drained by
        // its guard when the overflow error returns below.
        if self.var_args.is_some() {
            namespace[namespace_base + total_positional_params] = allocate_tuple(pos_iter.collect(), vm.heap);
        }

        // 3. Bind keyword args
        // Bind keywords to args and kwargs (not pos_args - those are positional-only)
        let mut excess_kwargs_guard = DropGuard::new(self.var_kwargs.is_some().then(Dict::new), vm);
        let (excess_kwargs, vm) = excess_kwargs_guard.as_parts_mut();

        'kwargs: for (key, value) in keyword_args {
            // Guard key: dropped on most paths, consumed into **kwargs via into_parts().
            let mut key_guard = DropGuard::new(key, vm);
            let (key, vm) = key_guard.as_parts_mut();

            // Guard value: consumed into namespace/excess_kwargs via into_inner(),
            // or dropped automatically on error paths.
            let mut value_guard = DropGuard::new(value, vm);
            let vm = value_guard.ctx();

            let Some(keyword_name) = key.as_either_str(vm.heap) else {
                return Err(ExcType::type_error("keywords must be strings"));
            };

            // Check if this keyword matches a positional-only param (error)
            if let Some(pos_args) = &self.pos_args
                && let Some(&param_id) = pos_args
                    .iter()
                    .find(|&&param_id| keyword_name.matches(param_id, vm.interns))
            {
                let func = vm.interns.get_str(func_name.name_id);
                let param = vm.interns.get_str(param_id);
                return Err(ExcType::type_error_positional_only(func, param));
            }

            // Try positional-or-keyword params
            if let Some(args) = &self.args {
                for (i, &param_id) in args.iter().enumerate() {
                    if keyword_name.matches(param_id, vm.interns) {
                        let ns_idx = pos_param_count + i;
                        if !matches!(namespace[namespace_base + ns_idx], Value::Undefined) {
                            let func = vm.interns.get_str(func_name.name_id);
                            let param = vm.interns.get_str(param_id);
                            return Err(ExcType::type_error_duplicate_arg(func, param));
                        }
                        let (value, _) = value_guard.into_parts();
                        namespace[namespace_base + ns_idx] = value;
                        continue 'kwargs;
                    }
                }
            }

            // Try keyword-only params
            if let Some(kwargs) = &self.kwargs {
                for (i, &param_id) in kwargs.iter().enumerate() {
                    if keyword_name.matches(param_id, vm.interns) {
                        let ns_idx = total_positional_params + var_args_offset + i;
                        if !matches!(namespace[namespace_base + ns_idx], Value::Undefined) {
                            let func = vm.interns.get_str(func_name.name_id);
                            let param = vm.interns.get_str(param_id);
                            return Err(ExcType::type_error_duplicate_arg(func, param));
                        }
                        let (value, _) = value_guard.into_parts();
                        namespace[namespace_base + ns_idx] = value;
                        continue 'kwargs;
                    }
                }
            }

            if let Some(excess_kwargs) = excess_kwargs {
                // Consume both value and key into **kwargs dict
                let (value, _) = value_guard.into_parts();
                let (key, vm) = key_guard.into_parts();
                excess_kwargs.set(key, value, vm)?;
                continue 'kwargs;
            }

            let func = vm.interns.get_str(func_name.name_id);
            let key_str = keyword_name.as_str(vm.interns);
            return Err(ExcType::type_error_unexpected_keyword(func, key_str));
        }

        // 3.4. Raise the deferred too-many-positional error now that every
        // kwarg has bound (or errored). CPython reports the range form when
        // some positional params have defaults, plus an `(and N keyword-only
        // argument(s))` suffix counting only the kw-only params *bound by the
        // call* — defaults have not been applied yet, exactly as in CPython's
        // `too_many_positional`.
        if positional_overflow {
            let kwonly_given = (0..self.kwarg_count())
                .filter(|&i| {
                    !matches!(
                        namespace[namespace_base + total_positional_params + var_args_offset + i],
                        Value::Undefined
                    )
                })
                .count();
            let func = vm.interns.get_str(func_name.name_id);
            return Err(ExcType::type_error_too_many_positional_range(
                func,
                self.required_positional_count(),
                total_positional_params,
                positional_count,
                kwonly_given,
            ));
        }

        // 3.5. Apply default values to unbound optional parameters
        // Defaults layout: [pos_defaults...][arg_defaults...][kwarg_defaults...]
        // Each section only contains defaults for params that have them.
        let mut default_idx = 0;

        // Apply pos_args defaults (optional params at the end of pos_args)
        if self.pos_defaults_count > 0 {
            let first_optional = pos_param_count - self.pos_defaults_count;
            for i in first_optional..pos_param_count {
                if matches!(namespace[namespace_base + i], Value::Undefined) {
                    namespace[namespace_base + i] = defaults[default_idx + (i - first_optional)].clone_with_heap(vm);
                }
            }
        }
        default_idx += self.pos_defaults_count;

        // Apply args defaults (optional params at the end of args)
        if self.arg_defaults_count > 0 {
            let first_optional = arg_param_count - self.arg_defaults_count;
            for i in first_optional..arg_param_count {
                let ns_idx = pos_param_count + i;
                if matches!(namespace[namespace_base + ns_idx], Value::Undefined) {
                    namespace[namespace_base + ns_idx] =
                        defaults[default_idx + (i - first_optional)].clone_with_heap(vm);
                }
            }
        }
        default_idx += self.arg_defaults_count;

        // Apply kwargs defaults using the explicit default map
        if let Some(ref default_map) = self.kwarg_default_map {
            for (i, default_slot) in default_map.iter().enumerate() {
                if let Some(slot_idx) = default_slot {
                    // Skip past *args slot if present
                    let ns_idx = total_positional_params + var_args_offset + i;
                    if matches!(namespace[namespace_base + ns_idx], Value::Undefined) {
                        namespace[namespace_base + ns_idx] = defaults[default_idx + slot_idx].clone_with_heap(vm);
                    }
                }
            }
        }

        // 4. Check that all required params are bound BEFORE building final namespace.
        // This ensures we can clean up properly on error without leaking heap values.

        // Check required positional params (pos_args + required args)
        let mut missing_positional: Vec<&str> = Vec::new();

        // Check pos_args
        if let Some(ref pos_args) = self.pos_args {
            let required_pos_only = pos_args.len().saturating_sub(self.pos_defaults_count);
            for (i, &param_id) in pos_args.iter().enumerate() {
                if i < required_pos_only && matches!(namespace[namespace_base + i], Value::Undefined) {
                    missing_positional.push(vm.interns.get_str(param_id));
                }
            }
        }

        // Check args (positional-or-keyword)
        if let Some(ref args_params) = self.args {
            let required_args = args_params.len().saturating_sub(self.arg_defaults_count);
            for (i, &param_id) in args_params.iter().enumerate() {
                if i < required_args && matches!(namespace[namespace_base + pos_param_count + i], Value::Undefined) {
                    missing_positional.push(vm.interns.get_str(param_id));
                }
            }
        }

        if !missing_positional.is_empty() {
            // Clean up bound values before returning error
            let func = vm.interns.get_str(func_name.name_id);
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
                let ns_idx = total_positional_params + var_args_offset + i;
                if !has_default && matches!(namespace[namespace_base + ns_idx], Value::Undefined) {
                    missing_kwonly.push(vm.interns.get_str(param_id));
                }
            }
        }

        if !missing_kwonly.is_empty() {
            let func = vm.interns.get_str(func_name.name_id);
            return Err(ExcType::type_error_missing_kwonly_with_names(func, &missing_kwonly));
        }

        // 5. Insert **kwargs dict if present (at the last slot)
        // Namespace layout: [pos_args][args][*args?][kwargs][**kwargs?]
        let (excess_kwargs, vm) = excess_kwargs_guard.into_parts();
        if let Some(excess_kwargs) = excess_kwargs {
            let dict_id = vm.heap.allocate(HeapData::Dict(excess_kwargs));
            let last_slot = namespace.len() - 1;
            namespace[last_slot] = Value::Ref(dict_id);
        }

        Ok(())
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

    /// Returns the total number of default values across all parameter groups.
    pub fn total_defaults_count(&self) -> usize {
        self.pos_defaults_count + self.arg_defaults_count + self.kwarg_defaults_count()
    }

    /// Validates compiler-established default metadata before dump loading.
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        if self.pos_defaults_count > self.pos_arg_count() {
            return Err("positional-only default count exceeds parameter count");
        }
        if self.arg_defaults_count > self.arg_count() {
            return Err("positional default count exceeds parameter count");
        }

        match (&self.kwargs, &self.kwarg_default_map) {
            (None, None) => Ok(()),
            (Some(kwargs), Some(default_map)) if kwargs.len() == default_map.len() => {
                for (expected, &actual) in default_map.iter().flatten().enumerate() {
                    if actual != expected {
                        return Err("keyword-only default indices are not contiguous");
                    }
                }
                Ok(())
            }
            _ => Err("keyword-only parameters and default map have different lengths"),
        }
    }

    /// Injects malformed metadata for dump tests.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn corrupt_metadata_for_tests(&mut self, fault: SignatureMetadataFault) {
        match fault {
            SignatureMetadataFault::PosDefaultsCountOutOfRange => {
                self.pos_defaults_count = self.pos_arg_count() + 1;
            }
            SignatureMetadataFault::ArgDefaultsCountOutOfRange => {
                self.arg_defaults_count = self.arg_count() + 1;
            }
            SignatureMetadataFault::KwargDefaultMapLengthMismatch => {
                self.kwarg_default_map
                    .as_mut()
                    .expect("test function has keyword-only parameters")
                    .pop();
            }
            SignatureMetadataFault::KwargDefaultIndexGap => {
                *self
                    .kwarg_default_map
                    .as_mut()
                    .expect("test function has keyword-only parameters")
                    .iter_mut()
                    .flatten()
                    .next()
                    .expect("test function has a keyword-only default") = 1;
            }
        }
    }

    /// Returns the minimum number of positional arguments required.
    ///
    /// This is the total positional param count minus the number of defaults.
    /// For a signature like `def f(a, b, c=1)`, this returns 2 (a and b are required).
    #[inline]
    fn required_positional_count(&self) -> usize {
        self.pos_arg_count() + self.arg_count() - self.pos_defaults_count - self.arg_defaults_count
    }

    fn kwarg_defaults_count(&self) -> usize {
        self.kwarg_default_map
            .as_deref()
            .map(|v| v.iter().filter(|&x| x.is_some()).count())
            .unwrap_or_default()
    }

    /// Returns the number of positional-only parameters.
    fn pos_arg_count(&self) -> usize {
        self.pos_args.as_ref().map_or(0, Vec::len)
    }

    /// Returns the number of positional-or-keyword parameters.
    fn arg_count(&self) -> usize {
        self.args.as_ref().map_or(0, Vec::len)
    }

    /// Returns the number of keyword-only parameters.
    fn kwarg_count(&self) -> usize {
        self.kwargs.as_ref().map_or(0, Vec::len)
    }

    /// Returns an iterator over all parameter names in namespace slot order.
    ///
    /// Order: pos_args, args, var_args (if present), kwargs, var_kwargs (if present)
    fn param_names(&self) -> impl Iterator<Item = StringId> + '_ {
        let pos_args = self.pos_args.iter().flat_map(|v| v.iter().copied());
        let args = self.args.iter().flat_map(|v| v.iter().copied());
        let var_args = self.var_args.iter().copied();
        let kwargs = self.kwargs.iter().flat_map(|v| v.iter().copied());
        let var_kwargs = self.var_kwargs.iter().copied();

        pos_args.chain(args).chain(var_args).chain(kwargs).chain(var_kwargs)
    }

    /// Returns the maximum number of positional arguments accepted.
    ///
    /// Returns None if *args is present (unlimited positional args).
    fn max_positional_count(&self) -> Option<usize> {
        if self.var_args.is_some() {
            None
        } else {
            Some(self.pos_arg_count() + self.arg_count())
        }
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
        // Only reached from the Simple/SimpleWithDefaults fast paths, so there
        // are no pos-only, kw-only, or `*args` params: every param is
        // positional-or-keyword and `param_count` is the positional maximum.
        let required = self.required_positional_count();
        let msg = if actual_count < required {
            // Missing arguments — CPython lists only the *required* params not
            // yet provided (params with defaults are never "missing").
            let missing_names: Vec<String> = self
                .param_names()
                .take(required)
                .skip(actual_count)
                .map(|string_id| interns.get_str(string_id).to_string())
                .collect();
            let missing_refs: Vec<&str> = missing_names.iter().map(String::as_str).collect();
            ExcType::missing_positional_msg(name_str, &missing_refs)
        } else {
            ExcType::too_many_positional_range_msg(name_str, required, param_count, actual_count, 0)
        };
        Err(SimpleException::new_msg(ExcType::TypeError, msg)
            .with_position(func_name.position)
            .into())
    }
}
