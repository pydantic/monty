//! Binds native calls into raw slots before the generated adapter converts any values.
//!
//! The binder owns argument dispatch, arity checks and cleanup. The adapter supplies the
//! signature and handles conversions and defaults. User-defined functions bind through
//! the sibling module because their parameters and output slots use different types.

use std::mem;

use crate::{
    args::{ArgPosIter, ArgValues, KwargsValues, KwargsValuesIter},
    bytecode::VM,
    defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{ContainsHeap, DropWithContext},
    intern::{Interns, StaticStrings},
    value::{EitherStr, Value},
};

/// The general binding path: iterators, a cleanup guard, kwarg dispatch, and
/// arity checks. See [`ParamSpec::bind`] for the contract.
fn bind_slow<const N: usize>(
    spec: &ParamSpec<N>,
    name: &str,
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
        return Err(ExcType::kwargs_not_implemented(name));
    }

    let n_pos = state.pos.len();
    let positional_overflow = n_pos > spec.positional && spec.varargs.is_none();
    for slot in bound.slots.iter_mut().take(n_pos.min(spec.positional)) {
        *slot = state.pos.next();
    }
    if spec.varargs.is_some() {
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
            Some((_, param)) if matches!(param.kind, ParamKind::PosOnly(_)) => {
                (key, value).drop_with(vm);
                return Err(ExcType::type_error_positional_only(name, param.name()));
            }
            Some((idx, param)) => {
                key.drop_with(vm);
                if bound.slots[idx].is_some() {
                    value.drop_with(vm);
                    return Err(ExcType::type_error_duplicate_arg(name, param.name()));
                }
                bound.slots[idx] = Some(value);
            }
            None if spec.varkwargs.is_some() => {
                // The key is already known to be a string (`as_either_str`
                // succeeded above) — keep it as a `Value` so runtime-built
                // heap strings (e.g. `dict(**{k: 1})`) work, not just
                // interned ids.
                state.unmatched.push((key, value));
            }
            None => {
                value.drop_with(vm);
                let keyword = key_str.into_string(vm.interns);
                key.drop_with(vm);
                let err_name = spec.kwarg_error_name.unwrap_or(name);
                return Err(ExcType::type_error_unexpected_keyword(err_name, &keyword));
            }
        }
    }

    if positional_overflow {
        let receiver = usize::from(spec.bound_receiver);
        return Err(ExcType::type_error_takes_at_most(
            name,
            spec.positional + receiver,
            n_pos + receiver,
            spec.varkwargs.is_some() || spec.positional < spec.params.len(),
        ));
    }

    let missing = missing_names(spec, &bound.slots, 0, spec.positional);
    if !missing.is_empty() {
        return Err(ExcType::type_error_missing_positional_with_names(name, &missing));
    }
    let missing = missing_names(spec, &bound.slots, spec.positional, N);
    if !missing.is_empty() {
        return Err(ExcType::type_error_missing_kwonly_with_names(name, &missing));
    }

    if let Some(slot) = collector
        && !state.unmatched.is_empty()
    {
        *slot = KwargsValues::Pairs(mem::take(&mut state.unmatched));
    }
    Ok(())
}

pub(crate) struct Param {
    kind: ParamKind,
    required: bool,
}

#[derive(Clone, Copy)]
enum ParamKind {
    PosOnly(&'static str),
    Keyword(StaticStrings),
    KwOnly(StaticStrings),
}

impl Param {
    pub(crate) const fn pos_only(name: &'static str) -> Self {
        Self {
            kind: ParamKind::PosOnly(name),
            required: true,
        }
    }

    pub(crate) const fn keyword(name: StaticStrings) -> Self {
        Self {
            kind: ParamKind::Keyword(name),
            required: true,
        }
    }

    pub(crate) const fn kw_only(name: StaticStrings) -> Self {
        Self {
            kind: ParamKind::KwOnly(name),
            required: true,
        }
    }

    pub(crate) const fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    fn name(&self) -> &'static str {
        match self.kind {
            ParamKind::PosOnly(name) => name,
            ParamKind::Keyword(name) | ParamKind::KwOnly(name) => name.into(),
        }
    }

    fn keyword_name(&self) -> Option<StaticStrings> {
        match self.kind {
            ParamKind::Keyword(name) | ParamKind::KwOnly(name) => Some(name),
            ParamKind::PosOnly(_) => None,
        }
    }
}

/// Named parameters in declaration order. The array length is also the number of binding slots.
pub(crate) struct ParamSpec<const N: usize> {
    params: [Param; N],
    positional: usize,
    /// First positional count that fills every required slot. Exceeds `positional` for a required keyword-only slot.
    required_prefix: usize,
    varargs: Option<&'static str>,
    varkwargs: Option<&'static str>,
    has_keyword_slots: bool,
    bound_receiver: bool,
    kwarg_error_name: Option<&'static str>,
    kwargs_not_supported_yet: bool,
}

impl<const N: usize> ParamSpec<N> {
    pub(crate) const fn new(params: [Param; N]) -> Self {
        let mut positional = 0;
        let mut has_keyword_slots = false;
        let mut required_prefix = 0;
        let mut i = 0;
        while i < N {
            if !matches!(params[i].kind, ParamKind::KwOnly(_)) {
                assert!(
                    i == positional,
                    "positional parameters must precede keyword-only parameters"
                );
                positional += 1;
            }
            has_keyword_slots |= !matches!(params[i].kind, ParamKind::PosOnly(_));
            if params[i].required {
                required_prefix = i + 1;
            }
            i += 1;
        }
        Self {
            params,
            positional,
            required_prefix,
            varargs: None,
            varkwargs: None,
            has_keyword_slots,
            bound_receiver: false,
            kwarg_error_name: None,
            kwargs_not_supported_yet: false,
        }
    }

    pub(crate) const fn varargs(mut self, name: &'static str) -> Self {
        self.varargs = Some(name);
        self
    }

    pub(crate) const fn varkwargs(mut self, name: &'static str) -> Self {
        self.varkwargs = Some(name);
        self
    }

    pub(crate) const fn kwargs_not_supported_yet(mut self) -> Self {
        self.kwargs_not_supported_yet = true;
        self
    }

    /// Fills the caller's guarded slots, transferring unmatched keywords to the collector.
    #[inline]
    pub(crate) fn bind(
        &self,
        name: &str,
        bound: &mut Bound<N>,
        args: ArgValues,
        collector: Option<&mut KwargsValues>,
        vm: &mut VM<'_>,
    ) -> RunResult<()> {
        debug_assert_eq!(collector.is_some(), self.varkwargs.is_some());
        let (args, collector) = match (args, collector) {
            (ArgValues::Empty, _) if self.required_prefix == 0 => {
                return Ok(());
            }
            (ArgValues::One(v), _) if self.required_prefix <= 1 && self.positional >= 1 => {
                bound.slots[0] = Some(v);
                return Ok(());
            }
            (ArgValues::Two(v1, v2), _) if self.required_prefix <= 2 && self.positional >= 2 => {
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
            ) if self.forwards_keywords() && (self.required_prefix..=self.positional).contains(&args.len()) => {
                for (slot, value) in bound.slots.iter_mut().zip(args) {
                    *slot = Some(value);
                }
                *slot = kwargs;
                return Ok(());
            }
            other => other,
        };
        bind_slow(self, name, bound, args, collector, vm)
    }

    pub(crate) const fn bound_receiver(mut self) -> Self {
        self.bound_receiver = true;
        self
    }

    pub(crate) const fn kwarg_error_name(mut self, name: &'static str) -> Self {
        self.kwarg_error_name = Some(name);
        self
    }

    pub(crate) const fn forwards_keywords(&self) -> bool {
        !self.has_keyword_slots && !self.kwargs_not_supported_yet
    }
}

/// Raw bound arguments: the output of [`ParamSpec::bind`], consumed by generated code.
///
/// Holds heap references — every slot must be drained (via
/// [`require`](Self::require) / [`take`](Self::take) /
/// [`take_varargs`](Self::take_varargs))
/// or the whole value dropped with the heap. Generated code embeds it in the
/// guarded slots struct so both happen automatically.
pub(crate) struct Bound<const N: usize> {
    slots: [Option<Value>; N],
    varargs: Vec<Value>,
}

impl<const N: usize> Bound<N> {
    /// Creates empty slots for the generated adapter to fill.
    pub(crate) fn new() -> Self {
        Self {
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
fn find_param<'s, const N: usize>(
    spec: &'s ParamSpec<N>,
    key: &EitherStr,
    interns: &Interns,
) -> Option<(usize, &'s Param)> {
    let keyword = key.static_string(interns);
    spec.params.iter().enumerate().find(|(_, param)| {
        if matches!(param.kind, ParamKind::PosOnly(_)) {
            // A Python function can capture a positional-only name in **kwargs.
            spec.varkwargs.is_none() && param.name() == key.as_str(interns)
        } else {
            keyword.is_some() && param.keyword_name() == keyword
        }
    })
}

/// Collect the names of required-but-unfilled params in `start..end` (the
/// positional prefix or the keyword-only tail) for the aggregated missing errors.
fn missing_names<const N: usize>(
    spec: &ParamSpec<N>,
    slots: &[Option<Value>],
    start: usize,
    end: usize,
) -> Vec<&'static str> {
    spec.params[start..end]
        .iter()
        .zip(&slots[start..end])
        .filter(|(p, slot)| p.required && slot.is_none())
        .map(|(p, _)| p.name())
        .collect()
}
