//! Function signature representation and argument binding.
//!
//! This module handles Python function signatures including all parameter types:
//! positional-only, positional-or-keyword, *args, keyword-only, and **kwargs.
//! It also handles default values and the argument binding algorithm.
//!
//! # Design
//!
//! The `Signature` enum uses inline variants for common cases (0-3 simple parameters)
//! to avoid heap allocation and improve cache locality. Complex signatures with
//! defaults, *args, **kwargs, or more parameters use a boxed `ComplexSignature`.

use crate::{
    args::{ArgValues, KwargsValues},
    exception_private::{ExcType, RunResult, SimpleException},
    expressions::Identifier,
    heap::{Heap, HeapData},
    intern::{Interns, StringId},
    resource::ResourceTracker,
    types::{Dict, Tuple},
    value::Value,
};

/// Compact function signature representation.
///
/// Common cases use small inline variants to avoid heap allocation;
/// complex signatures use boxed storage. All variants enforce a practical
/// limit on parameter counts (up to 64 named parameters via bitmap tracking).
///
/// # Variants
///
/// - `Empty`: No parameters (`def f(): ...`)
/// - `One`: Single positional-or-keyword parameter (`def f(x): ...`)
/// - `Two`: Two positional-or-keyword parameters (`def f(x, y): ...`)
/// - `Three`: Three positional-or-keyword parameters (`def f(x, y, z): ...`)
/// - `Complex`: 4+ params, defaults, pos-only, *args, **kwargs, or kwonly
///
/// # Namespace Layout
///
/// Parameters are laid out in the namespace in this order:
/// ```text
/// [pos_only][pos_or_kw][*args_slot?][kwonly][**kwargs_slot?]
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub enum Signature {
    /// No parameters: `def f(): ...`
    #[default]
    Empty,

    /// Single positional-or-keyword parameter: `def f(x): ...`
    One(StringId),

    /// Two positional-or-keyword parameters: `def f(x, y): ...`
    Two(StringId, StringId),

    /// Three positional-or-keyword parameters: `def f(x, y, z): ...`
    Three(StringId, StringId, StringId),

    /// Full signature for complex cases: 4+ params, defaults, pos-only, *args, **kwargs, or kwonly
    Complex(Box<ComplexSignature>),
}

/// Full signature data for complex parameter configurations.
///
/// This is used when the signature has:
/// - 4 or more positional-or-keyword parameters
/// - Any default values
/// - Positional-only parameters (`/`)
/// - Variable positional parameter (`*args`)
/// - Keyword-only parameters
/// - Variable keyword parameter (`**kwargs`)
///
/// # Storage Layout
///
/// All parameter names are stored in a single `names` Vec in namespace order:
/// `[pos_only...][pos_or_kw...][kwonly...]`. The count fields indicate where
/// each section starts and ends.
///
/// # Default Values
///
/// Default values for pos-only and pos-or-kw params are tracked by count from the end.
/// Keyword-only defaults are tracked via a bitmap where bit `i` indicates if `kwonly[i]`
/// has a default. The actual default Values are stored separately in the function object.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ComplexSignature {
    /// All parameter names in namespace order: [pos_only][pos_or_kw][kwonly]
    names: Vec<StringId>,

    /// Number of positional-only parameters
    pos_only_count: u8,

    /// Number of positional-or-keyword parameters
    pos_or_kw_count: u8,

    /// Number of keyword-only parameters
    kw_only_count: u8,

    /// Number of pos-only params with defaults (from end)
    pos_defaults_count: u8,

    /// Number of pos-or-kw params with defaults (from end)
    arg_defaults_count: u8,

    /// *args parameter name (if present)
    var_args: Option<StringId>,

    /// **kwargs parameter name (if present)
    var_kwargs: Option<StringId>,

    /// Bitmap for which kwonly params have defaults.
    /// Bit `i` set means `kwonly[i]` has a default value.
    /// Supports up to 64 keyword-only parameters.
    kwarg_defaults_bitmap: u64,
}

impl Signature {
    /// Creates a signature from all parameter types.
    ///
    /// Automatically selects the most compact representation:
    /// - `Empty`/`One`/`Two`/`Three` for simple 0-3 param functions with no special features
    /// - `Complex` for everything else
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
    ///
    /// # Panics
    /// Panics if total named parameters exceed 64 (enforced for bitmap tracking).
    #[expect(clippy::too_many_arguments)]
    #[expect(clippy::cast_possible_truncation)]
    pub fn new(
        pos_args: &[StringId],
        pos_defaults_count: usize,
        args: &[StringId],
        arg_defaults_count: usize,
        var_args: Option<StringId>,
        kwargs: &[StringId],
        kwarg_default_map: &[Option<usize>],
        var_kwargs: Option<StringId>,
    ) -> Self {
        // Enforce 64-parameter limit for bitmap tracking
        let total_named = pos_args.len() + args.len() + kwargs.len();
        assert!(total_named <= 64, "functions cannot have more than 64 parameters");

        // Check if we can use an inline variant (simple signature with 0-3 params)
        let is_simple = pos_args.is_empty()
            && pos_defaults_count == 0
            && arg_defaults_count == 0
            && var_args.is_none()
            && kwargs.is_empty()
            && var_kwargs.is_none();

        if is_simple {
            match args.len() {
                0 => return Self::Empty,
                1 => return Self::One(args[0]),
                2 => return Self::Two(args[0], args[1]),
                3 => return Self::Three(args[0], args[1], args[2]),
                _ => {} // Fall through to Complex
            }
        }

        // Build the kwarg_defaults_bitmap from kwarg_default_map
        let mut kwarg_defaults_bitmap: u64 = 0;
        for (i, default_slot) in kwarg_default_map.iter().enumerate() {
            if default_slot.is_some() && i < 64 {
                kwarg_defaults_bitmap |= 1 << i;
            }
        }

        // Consolidate all names into a single Vec
        let mut names = Vec::with_capacity(pos_args.len() + args.len() + kwargs.len());
        names.extend(pos_args.iter().copied());
        names.extend(args.iter().copied());
        names.extend(kwargs.iter().copied());

        Self::Complex(Box::new(ComplexSignature {
            names,
            pos_only_count: pos_args.len() as u8,
            pos_or_kw_count: args.len() as u8,
            kw_only_count: kwargs.len() as u8,
            pos_defaults_count: pos_defaults_count as u8,
            arg_defaults_count: arg_defaults_count as u8,
            var_args,
            var_kwargs,
            kwarg_defaults_bitmap,
        }))
    }

    /// Returns true if this is a simple signature (no defaults, no *args/**kwargs).
    ///
    /// Simple signatures can use a fast path for argument binding that avoids
    /// the full binding algorithm overhead. A simple signature has:
    /// - No positional-only parameters
    /// - No defaults for any parameters
    /// - No *args or **kwargs
    /// - No keyword-only parameters
    #[inline]
    pub fn is_simple(&self) -> bool {
        matches!(
            self,
            Self::Empty | Self::One(_) | Self::Two(_, _) | Self::Three(_, _, _)
        )
    }

    /// Returns true if this signature has only positional-or-keyword params with defaults.
    ///
    /// This identifies the common pattern `def f(a, b=1, c=2)` where:
    /// - No positional-only parameters
    /// - No *args or **kwargs
    /// - No keyword-only parameters
    /// - Has some default values
    #[inline]
    fn is_simple_with_defaults(&self) -> bool {
        match self {
            Self::Empty | Self::One(_) | Self::Two(_, _) | Self::Three(_, _, _) => false,
            Self::Complex(c) => {
                c.pos_only_count == 0
                    && c.var_args.is_none()
                    && c.kw_only_count == 0
                    && c.var_kwargs.is_none()
                    && c.arg_defaults_count > 0
            }
        }
    }

    /// Returns the minimum number of positional arguments required.
    #[inline]
    fn required_positional_count(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::One(_) => 1,
            Self::Two(_, _) => 2,
            Self::Three(_, _, _) => 3,
            Self::Complex(c) => {
                let total = c.pos_only_count as usize + c.pos_or_kw_count as usize;
                total - c.pos_defaults_count as usize - c.arg_defaults_count as usize
            }
        }
    }

    /// Returns the total number of default values across all parameter groups.
    pub fn total_defaults_count(&self) -> usize {
        match self {
            Self::Empty | Self::One(_) | Self::Two(_, _) | Self::Three(_, _, _) => 0,
            Self::Complex(c) => {
                c.pos_defaults_count as usize
                    + c.arg_defaults_count as usize
                    + c.kwarg_defaults_bitmap.count_ones() as usize
            }
        }
    }

    /// Returns the number of positional-only parameters.
    #[inline]
    pub fn pos_arg_count(&self) -> usize {
        match self {
            Self::Empty | Self::One(_) | Self::Two(_, _) | Self::Three(_, _, _) => 0,
            Self::Complex(c) => c.pos_only_count as usize,
        }
    }

    /// Returns the number of positional-or-keyword parameters.
    #[inline]
    pub fn arg_count(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::One(_) => 1,
            Self::Two(_, _) => 2,
            Self::Three(_, _, _) => 3,
            Self::Complex(c) => c.pos_or_kw_count as usize,
        }
    }

    /// Returns the number of keyword-only parameters.
    #[inline]
    pub fn kwarg_count(&self) -> usize {
        match self {
            Self::Empty | Self::One(_) | Self::Two(_, _) | Self::Three(_, _, _) => 0,
            Self::Complex(c) => c.kw_only_count as usize,
        }
    }

    /// Returns the total number of named parameters (excluding *args/**kwargs slots).
    #[inline]
    pub fn param_count(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::One(_) => 1,
            Self::Two(_, _) => 2,
            Self::Three(_, _, _) => 3,
            Self::Complex(c) => c.names.len(),
        }
    }

    /// Returns the total number of namespace slots needed for parameters.
    ///
    /// This includes slots for:
    /// - All named parameters (pos_args + args + kwargs)
    /// - The *args tuple (if var_args is Some)
    /// - The **kwargs dict (if var_kwargs is Some)
    #[inline]
    pub fn total_slots(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::One(_) => 1,
            Self::Two(_, _) => 2,
            Self::Three(_, _, _) => 3,
            Self::Complex(c) => {
                let mut slots = c.names.len();
                if c.var_args.is_some() {
                    slots += 1;
                }
                if c.var_kwargs.is_some() {
                    slots += 1;
                }
                slots
            }
        }
    }

    /// Returns an iterator over all parameter names in namespace slot order.
    ///
    /// Order: pos_args, args, var_args (if present), kwargs, var_kwargs (if present)
    pub fn param_names(&self) -> ParamNamesIter<'_> {
        match self {
            Self::Empty => ParamNamesIter::Empty,
            Self::One(a) => ParamNamesIter::One(Some(*a)),
            Self::Two(a, b) => ParamNamesIter::Two(Some(*a), Some(*b)),
            Self::Three(a, b, c) => ParamNamesIter::Three(Some(*a), Some(*b), Some(*c)),
            Self::Complex(sig) => ParamNamesIter::Complex(ComplexParamNamesIter::new(sig)),
        }
    }

    /// Returns the maximum number of positional arguments accepted.
    ///
    /// Returns None if *args is present (unlimited positional args).
    #[inline]
    pub fn max_positional_count(&self) -> Option<usize> {
        match self {
            Self::Empty => Some(0),
            Self::One(_) => Some(1),
            Self::Two(_, _) => Some(2),
            Self::Three(_, _, _) => Some(3),
            Self::Complex(c) => {
                if c.var_args.is_some() {
                    None
                } else {
                    Some(c.pos_only_count as usize + c.pos_or_kw_count as usize)
                }
            }
        }
    }

    // Helper accessors for the Complex variant fields (used in bind)

    #[inline]
    fn pos_defaults_count(&self) -> usize {
        match self {
            Self::Empty | Self::One(_) | Self::Two(_, _) | Self::Three(_, _, _) => 0,
            Self::Complex(c) => c.pos_defaults_count as usize,
        }
    }

    #[inline]
    fn arg_defaults_count(&self) -> usize {
        match self {
            Self::Empty | Self::One(_) | Self::Two(_, _) | Self::Three(_, _, _) => 0,
            Self::Complex(c) => c.arg_defaults_count as usize,
        }
    }

    #[inline]
    fn has_var_args(&self) -> bool {
        match self {
            Self::Empty | Self::One(_) | Self::Two(_, _) | Self::Three(_, _, _) => false,
            Self::Complex(c) => c.var_args.is_some(),
        }
    }

    #[inline]
    fn has_var_kwargs(&self) -> bool {
        match self {
            Self::Empty | Self::One(_) | Self::Two(_, _) | Self::Three(_, _, _) => false,
            Self::Complex(c) => c.var_kwargs.is_some(),
        }
    }

    /// Returns the positional-only parameter names slice (empty for inline variants).
    #[inline]
    fn pos_args_slice(&self) -> &[StringId] {
        match self {
            Self::Empty | Self::One(_) | Self::Two(_, _) | Self::Three(_, _, _) => &[],
            Self::Complex(c) => &c.names[..c.pos_only_count as usize],
        }
    }

    /// Returns the keyword-only parameter names slice (empty for inline variants).
    #[inline]
    fn kwargs_slice(&self) -> &[StringId] {
        match self {
            Self::Empty | Self::One(_) | Self::Two(_, _) | Self::Three(_, _, _) => &[],
            Self::Complex(c) => {
                let start = c.pos_only_count as usize + c.pos_or_kw_count as usize;
                &c.names[start..]
            }
        }
    }

    /// Returns true if kwonly param at index `i` has a default.
    #[inline]
    fn kwarg_has_default(&self, i: usize) -> bool {
        match self {
            Self::Empty | Self::One(_) | Self::Two(_, _) | Self::Three(_, _, _) => false,
            Self::Complex(c) => (c.kwarg_defaults_bitmap & (1 << i)) != 0,
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
        // Fast path for simple signatures (no defaults, no special params) and
        // signatures with only positional-or-keyword params and defaults.
        // This avoids the full binding algorithm overhead for common cases.
        let is_simple = self.is_simple();
        let is_simple_with_defaults = self.is_simple_with_defaults();

        if is_simple || is_simple_with_defaults {
            // Try to consume args directly into namespace without the full algorithm.
            // Returns Some(args) if kwargs were passed (need full algorithm).
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
                // Kwargs were passed - need full algorithm
                args = continue_args;
            } else {
                let actual_count = namespace.len();
                let param_count = self.param_count();

                if actual_count == param_count {
                    // Exact match - no defaults needed
                    return Ok(());
                }

                if is_simple_with_defaults {
                    let required = self.required_positional_count();
                    if actual_count >= required && actual_count < param_count {
                        // Apply defaults for remaining parameters
                        // Defaults are stored at the end of the defaults array for pos-or-kw params
                        let defaults_needed = param_count - actual_count;
                        let defaults_start = self.arg_defaults_count() - defaults_needed;
                        for i in 0..defaults_needed {
                            namespace.push(defaults[defaults_start + i].clone_with_heap(heap));
                        }
                        return Ok(());
                    }
                }

                // Wrong number of arguments - clean up and return error
                for val in namespace.drain(..) {
                    val.drop_with_heap(heap);
                }
                return self.wrong_arg_count_error(actual_count, interns, func_name);
            }
        }
        // Full binding algorithm for complex signatures or kwargs

        // Split args into positional iterator and keyword components without allocating
        let (mut pos_iter, keyword_args) = args.into_parts();

        // Calculate how many positional params we have
        let pos_param_count = self.pos_arg_count();
        let arg_param_count = self.arg_count();
        let total_positional_params = pos_param_count + arg_param_count;

        // Check positional argument count against maximum
        let positional_count = pos_iter.len();
        let kwonly_given = keyword_args.len();
        if let Some(max) = self.max_positional_count() {
            if positional_count > max {
                let func = interns.get_str(func_name.name_id);
                // Must clean up iterator and kwargs before returning error
                pos_iter.drop_remaining_with_heap(heap);
                keyword_args.drop_with_heap(heap);
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
        let var_args_offset = usize::from(self.has_var_args());
        for _ in 0..self.total_slots() {
            namespace.push(Value::Undefined);
        }

        // Track which parameters have been bound (for duplicate detection)
        // Uses a u64 bitmap - supports up to 64 named parameters which is sufficient
        // for any reasonable Python function (Python itself has practical limits).
        // Note: this tracks only named params, not *args/**kwargs slots
        let mut bound_params: u64 = 0;

        // 1. Bind positional args to pos_args, then args

        // Bind to pos_args
        for (i, slot) in namespace.iter_mut().enumerate().take(pos_param_count) {
            if let Some(val) = pos_iter.next() {
                *slot = val;
                bound_params |= 1 << i;
            }
        }

        // Bind to args
        for (i, slot) in namespace
            .iter_mut()
            .enumerate()
            .take(total_positional_params)
            .skip(pos_param_count)
        {
            if let Some(val) = pos_iter.next() {
                *slot = val;
                bound_params |= 1 << i;
            }
        }

        // 2. Collect excess positional args into *args tuple
        let excess_positional: Vec<Value> = pos_iter.collect();
        let var_args_value = if self.has_var_args() {
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
            let pos_args = self.pos_args_slice();
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

            // Use Option to track the value as we try to bind it
            let mut remaining_value = Some(value);
            let mut key_value = Some(key);

            // Try to bind to an args param (positional-or-keyword)
            // For inline variants (One/Two/Three), we need special handling
            let args_match = match self {
                Self::Empty => None,
                Self::One(a) => {
                    if keyword_name.matches(*a, interns) {
                        Some((0, *a))
                    } else {
                        None
                    }
                }
                Self::Two(a, b) => {
                    if keyword_name.matches(*a, interns) {
                        Some((0, *a))
                    } else if keyword_name.matches(*b, interns) {
                        Some((1, *b))
                    } else {
                        None
                    }
                }
                Self::Three(a, b, c) => {
                    if keyword_name.matches(*a, interns) {
                        Some((0, *a))
                    } else if keyword_name.matches(*b, interns) {
                        Some((1, *b))
                    } else if keyword_name.matches(*c, interns) {
                        Some((2, *c))
                    } else {
                        None
                    }
                }
                Self::Complex(sig) => {
                    let start = sig.pos_only_count as usize;
                    let end = start + sig.pos_or_kw_count as usize;
                    sig.names[start..end]
                        .iter()
                        .enumerate()
                        .find(|&(_, &param_id)| keyword_name.matches(param_id, interns))
                        .map(|(i, &param_id)| (i, param_id))
                }
            };

            if let Some((i, param_id)) = args_match {
                let idx = pos_param_count + i;
                if (bound_params & (1 << idx)) != 0 {
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
                bound_params |= 1 << idx;
                if let Some(key) = key_value.take() {
                    key.drop_with_heap(heap);
                }
            }

            // Try to bind to a kwargs param (keyword-only)
            if remaining_value.is_some() {
                let kwargs = self.kwargs_slice();
                for (i, &param_id) in kwargs.iter().enumerate() {
                    if keyword_name.matches(param_id, interns) {
                        // Skip past *args slot if present
                        let ns_idx = total_positional_params + var_args_offset + i;
                        let idx = total_positional_params + i;
                        if (bound_params & (1 << idx)) != 0 {
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
                        bound_params |= 1 << idx;
                        if let Some(bound_key) = key_value.take() {
                            bound_key.drop_with_heap(heap);
                        }
                        break;
                    }
                }
            }

            // If still not bound, handle as excess or error
            if let Some(v) = remaining_value {
                if self.has_var_kwargs() {
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
        let pos_defaults = self.pos_defaults_count();
        if pos_defaults > 0 {
            let first_optional = pos_param_count - pos_defaults;
            for i in first_optional..pos_param_count {
                if (bound_params & (1 << i)) == 0 {
                    namespace[i] = defaults[default_idx + (i - first_optional)].clone_with_heap(heap);
                    bound_params |= 1 << i;
                }
            }
        }
        default_idx += pos_defaults;

        // Apply args defaults (optional params at the end of args)
        let arg_defaults = self.arg_defaults_count();
        if arg_defaults > 0 {
            let first_optional = arg_param_count - arg_defaults;
            for i in first_optional..arg_param_count {
                let ns_idx = pos_param_count + i;
                if (bound_params & (1 << ns_idx)) == 0 {
                    namespace[ns_idx] = defaults[default_idx + (i - first_optional)].clone_with_heap(heap);
                    bound_params |= 1 << ns_idx;
                }
            }
        }
        default_idx += arg_defaults;

        // Apply kwargs defaults using the bitmap
        let kwonly_count = self.kwarg_count();
        let mut kwarg_default_offset = 0;
        for i in 0..kwonly_count {
            if self.kwarg_has_default(i) {
                let bound_idx = total_positional_params + i;
                // Skip past *args slot if present
                let ns_idx = total_positional_params + var_args_offset + i;
                if (bound_params & (1 << bound_idx)) == 0 {
                    namespace[ns_idx] = defaults[default_idx + kwarg_default_offset].clone_with_heap(heap);
                    bound_params |= 1 << bound_idx;
                }
                kwarg_default_offset += 1;
            }
        }

        // 4. Check that all required params are bound BEFORE building final namespace.
        // This ensures we can clean up properly on error without leaking heap values.
        let func = interns.get_str(func_name.name_id);

        // Check required positional params (pos_args + required args)
        let mut missing_positional: Vec<&str> = Vec::new();

        // Check pos_args (positional-only)
        let pos_args = self.pos_args_slice();
        let required_pos_only = pos_args.len().saturating_sub(pos_defaults);
        for (i, &param_id) in pos_args.iter().enumerate() {
            if i < required_pos_only && (bound_params & (1 << i)) == 0 {
                missing_positional.push(interns.get_str(param_id));
            }
        }

        // Check args (positional-or-keyword) - need special handling for inline variants
        let required_args = arg_param_count.saturating_sub(arg_defaults);
        match self {
            Self::Empty => {}
            Self::One(a) => {
                if (bound_params & 1) == 0 {
                    missing_positional.push(interns.get_str(*a));
                }
            }
            Self::Two(a, b) => {
                if (bound_params & 1) == 0 {
                    missing_positional.push(interns.get_str(*a));
                }
                if (bound_params & 2) == 0 {
                    missing_positional.push(interns.get_str(*b));
                }
            }
            Self::Three(a, b, c) => {
                if (bound_params & 1) == 0 {
                    missing_positional.push(interns.get_str(*a));
                }
                if (bound_params & 2) == 0 {
                    missing_positional.push(interns.get_str(*b));
                }
                if (bound_params & 4) == 0 {
                    missing_positional.push(interns.get_str(*c));
                }
            }
            Self::Complex(sig) => {
                let start = sig.pos_only_count as usize;
                let end = start + sig.pos_or_kw_count as usize;
                for (i, &param_id) in sig.names[start..end].iter().enumerate() {
                    if i < required_args && (bound_params & (1 << (pos_param_count + i))) == 0 {
                        missing_positional.push(interns.get_str(param_id));
                    }
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
        let kwargs = self.kwargs_slice();
        for (i, &param_id) in kwargs.iter().enumerate() {
            let has_default = self.kwarg_has_default(i);
            if !has_default && (bound_params & (1 << (total_positional_params + i))) == 0 {
                missing_kwonly.push(interns.get_str(param_id));
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
        if self.has_var_kwargs() {
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
        Err(SimpleException::new_msg(ExcType::TypeError, msg)
            .with_position(func_name.position)
            .into())
    }
}

/// Iterator over parameter names in namespace slot order.
///
/// This handles both inline signature variants (Empty, One, Two, Three) and
/// Complex signatures. For Complex signatures, it yields names in order:
/// pos_only, pos_or_kw, var_args (if present), kwonly, var_kwargs (if present).
pub enum ParamNamesIter<'a> {
    Empty,
    One(Option<StringId>),
    Two(Option<StringId>, Option<StringId>),
    Three(Option<StringId>, Option<StringId>, Option<StringId>),
    Complex(ComplexParamNamesIter<'a>),
}

impl Iterator for ParamNamesIter<'_> {
    type Item = StringId;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Empty => None,
            Self::One(a) => a.take(),
            Self::Two(a, b) => a.take().or_else(|| b.take()),
            Self::Three(a, b, c) => a.take().or_else(|| b.take()).or_else(|| c.take()),
            Self::Complex(iter) => iter.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = match self {
            Self::Empty => 0,
            Self::One(a) => usize::from(a.is_some()),
            Self::Two(a, b) => usize::from(a.is_some()) + usize::from(b.is_some()),
            Self::Three(a, b, c) => usize::from(a.is_some()) + usize::from(b.is_some()) + usize::from(c.is_some()),
            Self::Complex(iter) => return iter.size_hint(),
        };
        (n, Some(n))
    }
}

/// Iterator over parameter names for Complex signatures.
///
/// Yields names in namespace order: pos_only, pos_or_kw, var_args, kwonly, var_kwargs.
pub struct ComplexParamNamesIter<'a> {
    sig: &'a ComplexSignature,
    /// Current position: 0 = named params, 1 = var_args, 2 = done after var_args, 3 = var_kwargs
    phase: u8,
    /// Index within the current phase (for named params)
    index: usize,
}

impl<'a> ComplexParamNamesIter<'a> {
    fn new(sig: &'a ComplexSignature) -> Self {
        Self {
            sig,
            phase: 0,
            index: 0,
        }
    }
}

impl Iterator for ComplexParamNamesIter<'_> {
    type Item = StringId;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.phase {
                0 => {
                    // Named params: pos_only + pos_or_kw
                    let named_count = self.sig.pos_only_count as usize + self.sig.pos_or_kw_count as usize;
                    if self.index < named_count {
                        let name = self.sig.names[self.index];
                        self.index += 1;
                        return Some(name);
                    }
                    self.phase = 1;
                    self.index = 0;
                }
                1 => {
                    // var_args
                    self.phase = 2;
                    if let Some(var_args) = self.sig.var_args {
                        return Some(var_args);
                    }
                }
                2 => {
                    // kwonly params
                    let kwonly_start = self.sig.pos_only_count as usize + self.sig.pos_or_kw_count as usize;
                    let kwonly_end = kwonly_start + self.sig.kw_only_count as usize;
                    if kwonly_start + self.index < kwonly_end {
                        let name = self.sig.names[kwonly_start + self.index];
                        self.index += 1;
                        return Some(name);
                    }
                    self.phase = 3;
                }
                3 => {
                    // var_kwargs
                    self.phase = 4;
                    if let Some(var_kwargs) = self.sig.var_kwargs {
                        return Some(var_kwargs);
                    }
                }
                _ => return None,
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let total = self.sig.names.len()
            + usize::from(self.sig.var_args.is_some())
            + usize::from(self.sig.var_kwargs.is_some());
        let consumed = match self.phase {
            0 => self.index,
            1 => self.sig.pos_only_count as usize + self.sig.pos_or_kw_count as usize,
            2 => {
                self.sig.pos_only_count as usize
                    + self.sig.pos_or_kw_count as usize
                    + usize::from(self.sig.var_args.is_some())
                    + self.index
            }
            3 => self.sig.names.len() + usize::from(self.sig.var_args.is_some()),
            _ => total,
        };
        let remaining = total.saturating_sub(consumed);
        (remaining, Some(remaining))
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
