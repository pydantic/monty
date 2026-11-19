//! Binds native calls into raw slots before the generated adapter converts any values.
//!
//! The binder owns argument dispatch, arity checks and cleanup. The adapter supplies the
//! signature and handles conversions and defaults. User-defined functions bind through
//! the sibling module because their parameters and output slots use different types.

use std::{mem, ptr};

use crate::{
    args::{ArgPosIter, ArgValues, KwargsValues, KwargsValuesIter},
    bytecode::VM,
    defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{ContainsHeap, DropWithContext},
    intern::{Interns, StaticStrings},
    value::{EitherStr, Value},
};

/// Bind positional and keyword arguments into `bound`'s raw [`Value`] slots
/// according to its signature.
///
/// Fills in place rather than returning a `Bound` so the (potentially large)
/// slot struct is never moved by value: the generated code allocates it once
/// inside its `DropGuard`-protected slots struct, and that caller guard also
/// owns cleanup of everything bound so far — on error, `bind` guarantees each
/// argument was either stored in `bound` or dropped with the heap, never
/// leaked in between.
///
/// The purely-positional 0/1/2-argument shapes — the overwhelming majority of
/// real calls — take a fast path that fills the slots directly: with no kwargs
/// and the count inside `n_required_positional..=n_positional`, *no* check in
/// the slow path can fire (every pre-check needs kwargs or an out-of-range
/// count, the derive guarantees required positional params precede defaulted
/// ones — so the first `n` slots are exactly the ones a valid call fills —
/// and the derive rejects required `kw_only` fields, so skipping the
/// aggregated missing-keyword check is safe). Keeping this function
/// `#[inline]` lets the shape dispatch fold into each derive's call site
/// while `bind_slow` stays outlined.
#[inline]
pub(crate) fn bind<const N: usize>(
    spec: &'static ParamSpec,
    bound: &mut Bound<N>,
    args: ArgValues,
    collector: Option<&mut KwargsValues>,
    vm: &mut VM<'_>,
) -> RunResult<()> {
    // `spec` is passed explicitly (despite living in `bound` too) so the
    // inlined fast-path conditions constant-fold against the caller's
    // `static` instead of loading through the guard-wrapped `bound`.
    debug_assert!(ptr::eq(spec, bound.spec), "spec must match the bound's spec");
    debug_assert_eq!(collector.is_some(), spec.varkwargs);
    let (args, collector) = match (args, collector) {
        (ArgValues::Empty, _) if spec.n_required_positional == 0 => {
            return Ok(());
        }
        (ArgValues::One(v), _) if spec.n_required_positional <= 1 && spec.n_positional >= 1 => {
            bound.slots[0] = Some(v);
            return Ok(());
        }
        (ArgValues::Two(v1, v2), _) if spec.n_required_positional <= 2 && spec.n_positional >= 2 => {
            bound.slots[0] = Some(v1);
            bound.slots[1] = Some(v2);
            return Ok(());
        }
        (
            ArgValues::ArgsKargs {
                args,
                kwargs: kwargs @ KwargsValues::Inline(_),
            },
            Some(slot),
        ) if spec.forwards_keywords() && (spec.n_required_positional..=spec.n_positional).contains(&args.len()) => {
            for (slot, value) in bound.slots.iter_mut().zip(args) {
                *slot = Some(value);
            }
            *slot = kwargs;
            return Ok(());
        }
        other => other,
    };
    bind_slow(spec, bound, args, collector, vm)
}

/// The general binding path: iterators, a cleanup guard, kwarg dispatch, and
/// arity checks. See [`bind`] for the contract.
fn bind_slow<const N: usize>(
    spec: &'static ParamSpec,
    bound: &mut Bound<N>,
    args: ArgValues,
    collector: Option<&mut KwargsValues>,
    vm: &mut VM<'_>,
) -> RunResult<()> {
    let (pos, kwargs) = args.into_parts();
    let state = IterState {
        pos,
        kwargs: kwargs.into_iter(),
        unmatched: Vec::new(),
    };
    // The guard drains both argument iterators on any `return Err(...)`;
    // values already moved into `bound` are covered by the *caller's* guard
    // around the slots struct, so no error arm needs manual drops beyond
    // values it has already pulled out of an iterator.
    defer_drop_mut!(state, vm);

    if spec.kwargs_not_supported_yet && state.kwargs.len() > 0 {
        return Err(ExcType::kwargs_not_implemented(spec.func_name));
    }

    let n_pos = state.pos.len();
    let positional_overflow = n_pos > spec.n_positional && !spec.varargs;
    for slot in bound.slots.iter_mut().take(n_pos.min(spec.n_positional)) {
        *slot = state.pos.next();
    }
    if spec.varargs {
        bound.varargs = match mem::replace(&mut state.pos, ArgPosIter::Empty) {
            ArgPosIter::Empty => Vec::new(),
            ArgPosIter::One(value) => vec![value],
            ArgPosIter::Two(values) => Vec::from(values),
            ArgPosIter::Vec(values) => values.collect(),
        };
    }

    for (key, value) in state.kwargs.by_ref() {
        let Some(key_str) = key.as_either_str(vm.heap) else {
            (key, value).drop_with(vm);
            return Err(ExcType::type_error_kwargs_nonstring_key());
        };
        match find_param(spec, &key_str, vm.interns) {
            Some((_, param)) if matches!(param.kind, ParamKind::PosOnly) => {
                (key, value).drop_with(vm);
                return Err(ExcType::type_error_positional_only(spec.func_name, param.name));
            }
            Some((idx, param)) => {
                key.drop_with(vm);
                if bound.slots[idx].is_some() {
                    value.drop_with(vm);
                    return Err(ExcType::type_error_duplicate_arg(spec.func_name, param.name));
                }
                bound.slots[idx] = Some(value);
            }
            None if spec.varkwargs => {
                // The key is already known to be a string (`as_either_str`
                // succeeded above) — keep it as a `Value` so runtime-built
                // heap strings (e.g. `dict(**{k: 1})`) work, not just
                // interned ids.
                state.unmatched.push((key, value));
            }
            None => {
                value.drop_with(vm);
                let name = key_str.into_string(vm.interns);
                key.drop_with(vm);
                let err_name = spec.kwarg_error_name.unwrap_or(spec.func_name);
                return Err(ExcType::type_error_unexpected_keyword(err_name, &name));
            }
        }
    }

    if positional_overflow {
        let receiver = usize::from(spec.bound_receiver);
        return Err(ExcType::type_error_takes_at_most(
            spec.func_name,
            spec.n_positional + receiver,
            n_pos + receiver,
            spec.varkwargs || spec.n_positional < spec.params.len(),
        ));
    }

    let missing = missing_names(spec, &bound.slots, 0, spec.n_positional);
    if !missing.is_empty() {
        return Err(ExcType::type_error_missing_positional_with_names(
            spec.func_name,
            &missing,
        ));
    }
    let missing = missing_names(spec, &bound.slots, spec.n_positional, N);
    if !missing.is_empty() {
        return Err(ExcType::type_error_missing_kwonly_with_names(spec.func_name, &missing));
    }

    if let Some(slot) = collector
        && !state.unmatched.is_empty()
    {
        *slot = KwargsValues::Pairs(mem::take(&mut state.unmatched));
    }
    Ok(())
}

/// Compile-time description of one `FromArgs` signature, generated as a
/// `static` by the derive and interpreted by [`bind`].
#[expect(clippy::struct_excessive_bools, reason = "mirrors independent signature axes")]
pub(crate) struct ParamSpec {
    /// Function name embedded in error messages (the `{name}()` prefix).
    pub func_name: &'static str,
    /// Include the bound receiver in positional overflow diagnostics.
    pub bound_receiver: bool,
    /// Named params in declaration order — `[pos_only..][pos_or_kw..][kw_only..]`.
    /// The index into this slice is also the slot index in [`Bound`].
    pub params: &'static [Param],
    /// Count of `PosOnly` + `PosOrKeyword` params (the positional slot prefix).
    pub n_positional: usize,
    /// Count of positional params without defaults.
    pub n_required_positional: usize,
    /// `*args` — excess positionals are collected instead of erroring.
    pub varargs: bool,
    /// `**kwargs` — unmatched kwargs are collected instead of erroring.
    pub varkwargs: bool,
    /// Reject any kwarg up front with `NotImplementedError` — a Monty TODO
    /// marker for functions whose CPython kwargs aren't plumbed through yet.
    pub kwargs_not_supported_yet: bool,
    /// Override for the function name in the unknown-kwarg error only
    /// (`json.dumps` reports `JSONEncoder.__init__`).
    pub kwarg_error_name: Option<&'static str>,
}

impl ParamSpec {
    /// Literal keyword names can go straight into the collector when no slot can match them.
    #[inline]
    pub(crate) fn forwards_keywords(&self) -> bool {
        self.params.iter().all(|param| param.keyword_name.is_none())
    }
}

/// One named parameter slot of a [`ParamSpec`].
pub(crate) struct Param {
    pub name: &'static str,
    /// Executor-independent identity used for keyword matching.
    pub keyword_name: Option<StaticStrings>,
    pub kind: ParamKind,
    /// True when the param has no default.
    pub required: bool,
}

/// Role of a named parameter. `*args` / `**kwargs` are spec-level flags, not
/// params — they own no slot.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParamKind {
    PosOnly,
    PosOrKeyword,
    KwOnly,
}

/// Raw bound arguments: the output of [`bind`], consumed by generated code.
///
/// Holds heap references — every slot must be drained (via
/// [`require`](Self::require) / [`take`](Self::take) /
/// [`take_varargs`](Self::take_varargs))
/// or the whole value dropped with the heap. Generated code embeds it in the
/// guarded slots struct so both happen automatically.
pub(crate) struct Bound<const N: usize> {
    spec: &'static ParamSpec,
    slots: [Option<Value>; N],
    varargs: Vec<Value>,
}

impl<const N: usize> Bound<N> {
    /// Empty slots for `spec`, ready for [`bind`]. Called by generated code
    /// when building its guarded slots struct; `N` must equal
    /// `spec.params.len()` (the derive guarantees this).
    pub(crate) fn new(spec: &'static ParamSpec) -> Self {
        debug_assert_eq!(N, spec.params.len(), "slot count must match spec params");
        Self {
            spec,
            slots: [const { None }; N],
            varargs: Vec::new(),
        }
    }

    /// Take a required argument. Successful binding guarantees the slot is filled.
    pub(crate) fn require(&mut self, i: usize) -> Value {
        self.slots[i].take().expect("required argument already checked")
    }

    /// Take optional param `i` (`None` = absent, caller applies the default).
    pub(crate) fn take(&mut self, i: usize) -> Option<Value> {
        self.slots[i].take()
    }

    /// Take the collected `*args` values.
    pub(crate) fn take_varargs(&mut self) -> Vec<Value> {
        mem::take(&mut self.varargs)
    }
}

impl<C: ContainsHeap, const N: usize> DropWithContext<C> for Bound<N> {
    fn drop_with(self, heap: &mut C) {
        for slot in self.slots {
            slot.drop_with(heap);
        }
        self.varargs.drop_with(heap);
    }
}

/// The partially-consumed argument iterators [`bind_slow`] owns mid-flight,
/// guarded so both are drained on every error path (already-bound values are
/// covered by the caller's guard around the slots struct).
struct IterState {
    pos: ArgPosIter,
    kwargs: KwargsValuesIter,
    unmatched: Vec<(Value, Value)>,
}

impl<C: ContainsHeap> DropWithContext<C> for IterState {
    fn drop_with(self, heap: &mut C) {
        self.pos.drop_with(heap);
        self.kwargs.drop_with(heap);
        self.unmatched.drop_with(heap);
    }
}

/// Finds the parameter named by a keyword in declaration order.
///
/// A positional-only name is recognized for diagnostics if there is no `**kwargs`.
fn find_param<'s>(spec: &'s ParamSpec, key: &EitherStr, interns: &Interns) -> Option<(usize, &'s Param)> {
    let keyword = key.static_string(interns);
    spec.params.iter().enumerate().find(|(_, param)| {
        if matches!(param.kind, ParamKind::PosOnly) {
            // A Python function can capture a positional-only name in **kwargs.
            !spec.varkwargs && param.name == key.as_str(interns)
        } else {
            keyword.is_some() && param.keyword_name == keyword
        }
    })
}

/// Collect the names of required-but-unfilled params in `start..end` (the
/// positional prefix or the keyword-only tail) for the aggregated missing errors.
fn missing_names(spec: &ParamSpec, slots: &[Option<Value>], start: usize, end: usize) -> Vec<&'static str> {
    spec.params[start..end]
        .iter()
        .zip(&slots[start..end])
        .filter(|(p, slot)| p.required && slot.is_none())
        .map(|(p, _)| p.name)
        .collect()
}
