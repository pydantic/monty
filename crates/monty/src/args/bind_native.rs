//! Runtime argument binder for *native* (Rust-implemented) callables,
//! driving `#[derive(FromArgs)]`.
//!
//! The derive macro emits a `static` [`ParamSpec`] describing the signature
//! and calls [`bind`], which does *all* of the dispatch work — arity
//! pre-checks, positional slot filling, kwarg matching, duplicate/conflict
//! detection, unknown-kwarg handling, `*args` / `**kwargs` collection, and
//! refcount cleanup on every error path — as ordinary, debuggable Rust.
//! Generated code is left with only the parts that must be compile-time:
//! per-field [`FromValue`](super::FromValue) conversions, default
//! expressions, and the final struct build.
//!
//! User-defined Python functions are bound by the sibling
//! [`bind_python`](super::bind_python) module instead. The two binders are
//! deliberately separate (different param representations, outputs, and
//! error families), but [`ErrorFamily::Def`] here MUST stay behaviourally
//! identical to `Signature::bind` — same kwargs-before-overflow ordering,
//! same wording, same `(and N keyword-only argument(s))` counting. When
//! changing `def` semantics in either file, change both; coverage lives in
//! `test_cases/args__macro_errors.py` and
//! `test_cases/function__arity_defaults.py`.
//!
//! No type conversion happens here: `bind` fills raw [`Value`] slots
//! ([`Bound`], in place) and the generated code drains them in declaration order via
//! [`Bound::require`] / [`Bound::take`], finishing with [`Bound::finish`].
//! That split is deliberate — it reproduces CPython's error orderings, which
//! differ per parser family (see [`ErrorFamily`]): the `def`/clinic/unpack
//! families detect *all* binding errors before any converter runs, while the
//! `PyArg_ParseTupleAndKeywords` families interleave missing errors with
//! conversion parameter-by-parameter and report leftover kwargs (conflicts,
//! unknowns) last — hence the deferred state on [`Bound`] raised by
//! [`Bound::finish`].

use std::{mem, ptr};

use crate::{
    ResourceTracker,
    args::{ArgPosIter, ArgValues, KwargsValues, KwargsValuesIter},
    bytecode::VM,
    exception_private::{ExcType, RunError, RunResult},
    heap::{ContainsHeap, DropGuard, DropWithContext},
    intern::{Interns, StringId},
    value::{EitherStr, Value},
};

/// Bind positional and keyword arguments into `bound`'s raw [`Value`] slots
/// per its spec, enforcing the spec's family arity/conflict/unknown-kwarg
/// ordering.
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
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<()> {
    // `spec` is passed explicitly (despite living in `bound` too) so the
    // inlined fast-path conditions constant-fold against the caller's
    // `static` instead of loading through the guard-wrapped `bound`.
    debug_assert!(ptr::eq(spec, bound.spec), "spec must match the bound's spec");
    let args = match args {
        ArgValues::Empty if spec.n_required_positional == 0 => {
            return Ok(());
        }
        ArgValues::One(v) if spec.n_required_positional <= 1 && spec.n_positional >= 1 => {
            bound.slots[0] = Some(v);
            return Ok(());
        }
        ArgValues::Two(v1, v2) if spec.n_required_positional <= 2 && spec.n_positional >= 2 => {
            bound.slots[0] = Some(v1);
            bound.slots[1] = Some(v2);
            return Ok(());
        }
        other => other,
    };
    bind_slow(spec, bound, args, vm)
}

/// The general binding path: iterators, a cleanup guard, kwarg dispatch, and
/// every family pre-check. See [`bind`] for the contract.
fn bind_slow<const N: usize>(
    spec: &'static ParamSpec,
    bound: &mut Bound<N>,
    args: ArgValues,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> RunResult<()> {
    let (pos, kwargs) = args.into_parts();
    let state = IterState {
        pos,
        kwargs: kwargs.into_iter(),
    };
    // The guard drains both argument iterators on any `return Err(...)`;
    // values already moved into `bound` are covered by the *caller's* guard
    // around the slots struct, so no error arm needs manual drops beyond
    // values it has already pulled out of an iterator.
    let mut guard = DropGuard::new(state, vm);
    let (state, vm) = guard.as_parts_mut();

    if spec.kwargs_not_supported_yet && state.kwargs.len() > 0 {
        return Err(ExcType::kwargs_not_implemented(spec.func_name));
    }

    let n_pos = state.pos.len();
    let n_kw = state.kwargs.len();

    // Family pre-checks. Order matches CPython: `PyArg_UnpackTuple` callers
    // reject keywords wholesale (`_PyArg_NoKeywords` / the `METH_FASTCALL`
    // dispatch) before any arity check, then check the positional range;
    // `at_most_total` reproduces the `PyArg_ParseTupleAndKeywords` total
    // pre-count; the "at least M positional" check reproduces
    // `_PyArg_UnpackKeywords` for C methods whose required positional-only
    // params cannot be filled by keyword.
    if matches!(spec.family, ErrorFamily::Unpack) {
        if n_kw > 0 {
            let name = spec.kwarg_error_name.unwrap_or(spec.func_name);
            return Err(ExcType::type_error_no_kwargs(name));
        }
        if let Some(err) = unpack_arity_error(spec, n_pos) {
            return Err(err);
        }
    }
    // `tp_vectorcall` fast paths (`int`, `str`) check positional arity with
    // `_PyArg_CheckPositional` before reaching the clinic parser, so kwarg-free
    // overflow gets the un-parenthesised wording; with kwargs present the
    // `at_most_total` check below fires with the clinic wording instead.
    if spec.vectorcall && n_kw == 0 && n_pos > spec.n_positional {
        return Err(ExcType::type_error_at_most(spec.func_name, spec.n_positional, n_pos));
    }
    if spec.at_most_total && n_pos + n_kw > spec.n_positional {
        return Err(total_overflow_error(spec, n_pos + n_kw));
    }
    if spec.uses_c_method_arity() && n_pos < spec.n_required_pos_only {
        return Err(ExcType::type_error_at_least_positional(
            spec.func_name,
            spec.n_required_pos_only,
            n_pos,
        ));
    }

    // Positional overflow: every C parser checks arity before touching kwargs,
    // but pure-Python `def` binding processes keyword arguments first — its
    // unexpected-kwarg and multiple-values errors beat too-many-positional —
    // so for `Def` the check is deferred to after the kwarg loop below.
    let positional_overflow = n_pos > spec.n_positional && !spec.varargs;
    if positional_overflow && !matches!(spec.family, ErrorFamily::Def) {
        return Err(positional_overflow_error(spec, n_pos, n_kw));
    }
    for slot in bound.slots.iter_mut().take(n_pos.min(spec.n_positional)) {
        *slot = state.pos.next();
    }
    // On (deferred) overflow without `*args` the excess stays in the iterator;
    // the guard drains it when the overflow error returns below.
    if spec.varargs {
        bound.varargs.extend(state.pos.by_ref());
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
                    match duplicate_error(spec, idx, param) {
                        DuplicateOutcome::Raise(err) => return Err(err),
                        // C families treat a kwarg naming an already-positional
                        // param as a *leftover*, reported by the final sweep
                        // (`finish`) — every missing/conversion error beats it,
                        // and among several conflicts CPython reports the
                        // earliest parameter (verified against 3.14).
                        DuplicateOutcome::Defer(err) => {
                            let deferred = bound.deferred_mut();
                            if deferred.conflict.as_ref().is_none_or(|(ci, _)| idx < *ci) {
                                deferred.conflict = Some((idx, err));
                            }
                        }
                    }
                } else {
                    bound.slots[idx] = Some(value);
                }
            }
            None if spec.varkwargs => {
                // The key is already known to be a string (`as_either_str`
                // succeeded above) — keep it as a `Value` so runtime-built
                // heap strings (e.g. `dict(**{k: 1})`) work, not just
                // interned ids.
                bound.varkwargs.push((key, value));
            }
            None if spec.family.defers_unknown_kwarg() => {
                // C families raise unknown-kwarg *last* (after every missing,
                // conversion, and conflict error); stash the first for `finish`.
                value.drop_with(vm);
                if bound.deferred.as_ref().is_none_or(|d| d.unknown.is_none()) {
                    let err = match spec.family {
                        ErrorFamily::C { .. } => ExcType::type_error_c_unexpected_keyword(key_str.as_str(vm.interns)),
                        _ => ExcType::type_error_unexpected_keyword(spec.func_name, key_str.as_str(vm.interns)),
                    };
                    bound.deferred_mut().unknown = Some(err);
                }
                key.drop_with(vm);
            }
            None => {
                value.drop_with(vm);
                let name = key_str.as_str(vm.interns).to_owned();
                key.drop_with(vm);
                let err_name = spec.kwarg_error_name.unwrap_or(spec.func_name);
                return Err(ExcType::type_error_unexpected_keyword(err_name, &name));
            }
        }
    }

    // The deferred `def` overflow: every kwarg bound cleanly, so report
    // too-many-positional with CPython's `(and N keyword-only argument(s))`
    // suffix, N = keyword-only slots *filled by the call* (CPython's
    // `too_many_positional` scans localsplus before defaults are applied).
    if positional_overflow {
        let kwonly_given = bound.slots[spec.n_positional..].iter().filter(|s| s.is_some()).count();
        return Err(ExcType::type_error_too_many_positional_range(
            spec.func_name,
            spec.n_required_positional,
            spec.n_positional,
            n_pos,
            kwonly_given,
        ));
    }

    // `def`/clinic/unpack report every missing required name in one aggregated
    // error, positional names first — before any conversion runs, exactly like
    // CPython's `def` binding and `_PyArg_UnpackKeywords`. The C families skip
    // this: their missing errors surface per-parameter from `Bound::require`,
    // interleaved with conversion, matching `vgetargskeywords`.
    if spec.family.aggregates_missing() {
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
    }

    // Both iterators are exhausted; the guard has nothing left to drain.
    Ok(())
}

/// Compile-time description of one `FromArgs` signature, generated as a
/// `static` by the derive and interpreted by [`bind`].
#[expect(clippy::struct_excessive_bools, reason = "mirrors independent signature axes")]
pub(crate) struct ParamSpec {
    /// Function name embedded in error messages (the `{name}()` prefix).
    pub func_name: &'static str,
    pub family: ErrorFamily,
    /// Named params in declaration order — `[pos_only..][pos_or_kw..][kw_only..]`.
    /// The index into this slice is also the slot index in [`Bound`].
    pub params: &'static [Param],
    /// Count of `PosOnly` + `PosOrKeyword` params (the positional slot prefix).
    pub n_positional: usize,
    /// Count of positional params without defaults.
    pub n_required_positional: usize,
    /// Count of required `PosOnly` params — non-zero switches the non-`def`
    /// families onto CPython's C-method `_PyArg_UnpackKeywords` arity wording
    /// (required positional-only args cannot be filled by keyword).
    pub n_required_pos_only: usize,
    /// `*args` — excess positionals are collected instead of erroring.
    pub varargs: bool,
    /// `**kwargs` — unmatched kwargs are collected instead of erroring.
    pub varkwargs: bool,
    /// Pre-count `positional + kwarg` against `n_positional` before dispatch,
    /// reproducing `PyArg_ParseTupleAndKeywords`' total pre-check. Set per
    /// function from CPython's observed behaviour — not derivable from the
    /// field shapes (identical signatures differ by parser generation).
    pub at_most_total: bool,
    /// Kwarg-free positional overflow uses `_PyArg_CheckPositional` wording
    /// (`{name} expected at most N arguments, got M`) — models `tp_vectorcall`
    /// fast paths (`int`, `str`) that bypass the clinic parser when no
    /// keywords are passed. Requires `at_most_total` (enforced by the derive).
    pub vectorcall: bool,
    /// Reject any kwarg up front with `NotImplementedError` — a Monty TODO
    /// marker for functions whose CPython kwargs aren't plumbed through yet.
    pub kwargs_not_supported_yet: bool,
    /// Override for the function name in the unknown-kwarg error only
    /// (`json.dumps` reports `JSONEncoder.__init__`).
    pub kwarg_error_name: Option<&'static str>,
}

impl ParamSpec {
    /// True when required positional-only params switch the arity wording to
    /// CPython's C-method form (`{name}() takes at least/most N positional
    /// arguments (M given)`). `def` keeps pure-Python wording even for
    /// required pos-only params (e.g. `json.loads(s, /)`); `unpack` has its
    /// own range pre-check.
    fn uses_c_method_arity(&self) -> bool {
        self.n_required_pos_only > 0 && !matches!(self.family, ErrorFamily::Def | ErrorFamily::Unpack)
    }
}

/// One named parameter slot of a [`ParamSpec`].
pub(crate) struct Param {
    pub name: &'static str,
    /// Interned id used for kwarg matching. `None` only for `pos_only` params
    /// without a `static_string` override — such params are not matchable by
    /// keyword and a kwarg with their name falls through to unknown-kwarg
    /// handling (rather than the "positional-only passed as keyword" error).
    pub kwarg_id: Option<StringId>,
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

/// CPython argument-parser family. Selects both the error *wording* and the
/// error *ordering* [`bind`] and [`Bound`] enforce; the derive's `style`
/// attribute maps onto this 1:1 (with `C`'s pivot computed from the fields).
#[derive(Clone, Copy)]
pub(crate) enum ErrorFamily {
    /// Pure-Python `def` binding (`style = def`). Too-many-positional counts
    /// positionals only and appends CPython's `(and N keyword-only
    /// argument(s))` suffix; all binding errors fire before the function body
    /// (i.e. before any conversion) — unknown kwargs immediately, missing
    /// names aggregated.
    Def,
    /// Argument Clinic `_PyArg_UnpackKeywords` (the default style). Binding
    /// errors fire before converters run: unknown kwargs and positional/keyword
    /// conflicts (`argument for {name}() given by name and position`)
    /// immediately, missing names aggregated; required positional-only params
    /// add the C-method "at least/at most N positional" wording.
    Clinic,
    /// `PyArg_ParseTupleAndKeywords` with no name in the format string —
    /// errors say `function takes …` / `this function got …`. Per-parameter
    /// ordering: missing → convert for each param in order; kwargs that name
    /// an already-positional param (conflicts) and unknown kwargs are both
    /// *leftovers*, raised last by [`Bound::finish`] — conflicts (earliest
    /// param) before unknowns, matching `vgetargskeywords`' final sweep.
    /// `positional_pivot` is computed by the derive (any `kw_only` field
    /// present) and switches the overflow wording to `… positional arguments …`
    /// with CPython's pivot back to the total count once the overflow exceeds
    /// all slots.
    C { positional_pivot: bool },
    /// Like [`ErrorFamily::C`] but the C source embeds the function name
    /// (`:name` in the format string) — errors say `{name}() …`, and the
    /// unknown-kwarg error uses the named Python wording.
    CNamed,
    /// `PyArg_UnpackTuple` (`style = unpack`): any keyword argument is
    /// rejected first with `{name}() takes no keyword arguments` (CPython's
    /// `_PyArg_NoKeywords` / `METH_FASTCALL` dispatch), then a fixed
    /// positional `min..max` range is checked, `{name} expected …` wording
    /// with no parentheses. When `min == max` CPython collapses to
    /// `expected N argument(s)` — [`bind`] does the same.
    Unpack,
}

impl ErrorFamily {
    /// C families raise unknown-kwarg after every missing/conversion error.
    fn defers_unknown_kwarg(self) -> bool {
        matches!(self, Self::C { .. } | Self::CNamed)
    }

    /// Non-C families aggregate all missing required names into one error at
    /// the end of [`bind`]; C families report per-param via [`Bound::require`].
    fn aggregates_missing(self) -> bool {
        !matches!(self, Self::C { .. } | Self::CNamed)
    }
}

/// Raw bound arguments: the output of [`bind`], consumed by generated code.
///
/// Holds heap references — every slot must be drained (via
/// [`require`](Self::require) / [`take`](Self::take) /
/// [`take_varargs`](Self::take_varargs) / [`take_varkwargs`](Self::take_varkwargs))
/// or the whole value dropped with the heap. Generated code embeds it in the
/// guarded slots struct so both happen automatically.
pub(crate) struct Bound<const N: usize> {
    spec: &'static ParamSpec,
    slots: [Option<Value>; N],
    varargs: Vec<Value>,
    varkwargs: Vec<(Value, Value)>,
    /// C families only: leftover-kwarg errors raised by [`finish`](Self::finish).
    /// Boxed because `RunError` is large and `Bound` is moved by value through
    /// the generated code — the common no-leftover call pays one niche'd word.
    deferred: Option<Box<DeferredLeftovers>>,
}

/// C-family leftover kwargs recorded during [`bind`], reported by
/// [`Bound::finish`] only after every missing/conversion error has had its
/// chance — matching `vgetargskeywords`' final sweep, where a conflict
/// (earliest parameter) beats an unknown kwarg.
#[derive(Default)]
struct DeferredLeftovers {
    /// Earliest-param positional/keyword conflict.
    conflict: Option<(usize, RunError)>,
    /// First unknown kwarg (in call order).
    unknown: Option<RunError>,
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
            varkwargs: Vec::new(),
            deferred: None,
        }
    }

    /// The deferred-leftovers slot, allocated on first use (C families only).
    fn deferred_mut(&mut self) -> &mut DeferredLeftovers {
        self.deferred.get_or_insert_default()
    }

    /// Take required param `i`, raising the family's missing-required error if
    /// absent. For the aggregating families the missing case is unreachable —
    /// [`bind`] already checked — so this only genuinely errors for `C`/`CNamed`.
    pub(crate) fn require(&mut self, i: usize) -> RunResult<Value> {
        match self.slots[i].take() {
            Some(v) => Ok(v),
            None => Err(self.missing_error(i)),
        }
    }

    /// Take optional param `i` (`None` = absent, caller applies the default).
    pub(crate) fn take(&mut self, i: usize) -> Option<Value> {
        self.slots[i].take()
    }

    /// Take the collected `*args` values.
    pub(crate) fn take_varargs(&mut self) -> Vec<Value> {
        mem::take(&mut self.varargs)
    }

    /// Take the collected `**kwargs`, collapsing empty to `KwargsValues::Empty`
    /// so callers get cheap emptiness checks.
    pub(crate) fn take_varkwargs(&mut self) -> KwargsValues {
        if self.varkwargs.is_empty() {
            KwargsValues::Empty
        } else {
            KwargsValues::Pairs(mem::take(&mut self.varkwargs))
        }
    }

    /// Raise the deferred C-family leftover-kwarg error, if any: the earliest-
    /// param conflict first, then the first unknown kwarg. Generated code
    /// calls this after every param has been required/taken, matching
    /// `vgetargskeywords`' final leftover sweep (every missing and conversion
    /// error beats a leftover). No-op for the other families, which raised
    /// immediately during [`bind`].
    pub(crate) fn finish(&mut self) -> RunResult<()> {
        match self.deferred.take() {
            None => Ok(()),
            Some(deferred) => match *deferred {
                DeferredLeftovers {
                    conflict: Some((_, err)),
                    ..
                } => Err(err),
                DeferredLeftovers { unknown: Some(err), .. } => Err(err),
                DeferredLeftovers {
                    conflict: None,
                    unknown: None,
                } => Ok(()),
            },
        }
    }

    /// Family-worded missing-required error for param `i`.
    #[cold]
    fn missing_error(&self, i: usize) -> RunError {
        let param = &self.spec.params[i];
        match self.spec.family {
            ErrorFamily::C { .. } if i < self.spec.n_positional => {
                ExcType::type_error_c_missing_required(param.name, i + 1)
            }
            ErrorFamily::CNamed if i < self.spec.n_positional => {
                ExcType::type_error_c_missing_required_named(self.spec.func_name, param.name, i + 1)
            }
            ErrorFamily::C { .. } | ErrorFamily::CNamed => {
                ExcType::type_error_missing_kwonly_with_names(self.spec.func_name, &[param.name])
            }
            // Aggregating families: bind() already raised; a required slot the
            // macro asks for is guaranteed filled. Emit the matching wording
            // anyway rather than panicking.
            _ => ExcType::type_error_missing_positional_with_names(self.spec.func_name, &[param.name]),
        }
    }
}

impl<C: ContainsHeap, const N: usize> DropWithContext<C> for Bound<N> {
    fn drop_with(self, heap: &mut C) {
        for slot in self.slots {
            slot.drop_with(heap);
        }
        self.varargs.drop_with(heap);
        for (k, v) in self.varkwargs {
            k.drop_with(heap);
            v.drop_with(heap);
        }
    }
}

/// The partially-consumed argument iterators [`bind_slow`] owns mid-flight,
/// guarded so both are drained on every error path (already-bound values are
/// covered by the caller's guard around the slots struct).
struct IterState {
    pos: ArgPosIter,
    kwargs: KwargsValuesIter,
}

impl<C: ContainsHeap> DropWithContext<C> for IterState {
    fn drop_with(self, heap: &mut C) {
        self.pos.drop_with(heap);
        self.kwargs.drop_with(heap);
    }
}

/// Find the param a kwarg key names, by matching interned ids in declaration
/// order. Params without a `kwarg_id` (plain pos-only) never match.
fn find_param<'s>(spec: &'s ParamSpec, key: &EitherStr, interns: &Interns) -> Option<(usize, &'s Param)> {
    spec.params
        .iter()
        .enumerate()
        .find(|(_, p)| p.kwarg_id.is_some_and(|id| key.matches(id, interns)))
}

/// How a duplicate (slot already filled) kwarg should be reported.
enum DuplicateOutcome {
    /// Raise immediately during bind (`def`/clinic/unpack, and all keyword-only
    /// duplicates — those can only arise from pathological `**` merges).
    Raise(RunError),
    /// C families: record as a leftover, raised by [`Bound::finish`].
    Defer(RunError),
}

/// Build the family-worded duplicate/conflict error for param `idx`.
/// `def` uses pure-Python binding's `got multiple values for argument`;
/// clinic uses `_PyArg_UnpackKeywords`' immediate `argument for {name}()
/// given by name and position` (verified against CPython 3.14 —
/// `'a'.encode(42, encoding='x')` reports the conflict, not the bad type);
/// the C families defer the same wording to the leftover sweep.
#[cold]
fn duplicate_error(spec: &ParamSpec, idx: usize, param: &Param) -> DuplicateOutcome {
    if matches!(param.kind, ParamKind::KwOnly) {
        return DuplicateOutcome::Raise(ExcType::type_error_multiple_values(spec.func_name, param.name));
    }
    let named_conflict =
        || ExcType::type_error_positional_keyword_conflict(&format!("{}()", spec.func_name), param.name, idx + 1);
    match spec.family {
        ErrorFamily::C { .. } => DuplicateOutcome::Defer(ExcType::type_error_positional_keyword_conflict(
            "function",
            param.name,
            idx + 1,
        )),
        ErrorFamily::CNamed => DuplicateOutcome::Defer(named_conflict()),
        ErrorFamily::Clinic => DuplicateOutcome::Raise(named_conflict()),
        ErrorFamily::Def | ErrorFamily::Unpack => {
            DuplicateOutcome::Raise(ExcType::type_error_duplicate_arg(spec.func_name, param.name))
        }
    }
}

/// `PyArg_UnpackTuple` range check: too few / too many positionals, with the
/// `expected N` collapse when `min == max` (exactly CPython's behaviour).
#[cold]
fn unpack_arity_error(spec: &ParamSpec, n_pos: usize) -> Option<RunError> {
    let (min, max) = (spec.n_required_positional, spec.n_positional);
    if n_pos < min {
        Some(if min == max {
            ExcType::type_error_expected_exact(spec.func_name, min, n_pos)
        } else {
            ExcType::type_error_at_least(spec.func_name, min, n_pos)
        })
    } else if n_pos > max {
        Some(if min == max {
            ExcType::type_error_expected_exact(spec.func_name, max, n_pos)
        } else {
            ExcType::type_error_at_most(spec.func_name, max, n_pos)
        })
    } else {
        None
    }
}

/// `at_most_total` pre-count error: `PyArg_ParseTupleAndKeywords`' total-count
/// wording for the C families, the parenthesised method form otherwise.
#[cold]
fn total_overflow_error(spec: &ParamSpec, total: usize) -> RunError {
    match spec.family {
        ErrorFamily::C {
            positional_pivot: false,
        } => ExcType::type_error_c_at_most(spec.n_positional, total),
        ErrorFamily::C { positional_pivot: true } => ExcType::type_error_c_at_most_positional(spec.n_positional, total),
        // Clinic / CNamed (`def`/`unpack` reject the flag at derive time).
        _ => ExcType::type_error_method_at_most(spec.func_name, spec.n_positional, total),
    }
}

/// Too-many-positional error for the non-`def` families without `*args`,
/// folding the kwarg count into the reported total (matching the C parsers'
/// "(M given)" figure). `def` never reaches here — its overflow is deferred
/// past the kwarg loop and worded inline in [`bind_slow`].
#[cold]
fn positional_overflow_error(spec: &ParamSpec, n_pos: usize, n_kw: usize) -> RunError {
    let max = spec.n_positional;
    match spec.family {
        // Unreachable: `def` defers, the unpack pre-check already covered both
        // directions. Match unpack's wording anyway rather than panicking.
        ErrorFamily::Def | ErrorFamily::Unpack => ExcType::type_error_at_most(spec.func_name, max, n_pos),
        _ if spec.uses_c_method_arity() => ExcType::type_error_method_at_most(spec.func_name, max, n_pos + n_kw),
        ErrorFamily::C {
            positional_pivot: false,
        } => ExcType::type_error_c_at_most(max, n_pos + n_kw),
        // CPython pivots from "M positional arguments" back to "M_total
        // arguments" once the overflow exceeds positional + kw-only slots.
        ErrorFamily::C { positional_pivot: true } => {
            ExcType::type_error_c_at_most_positional_or_total(max, spec.params.len(), n_pos + n_kw)
        }
        ErrorFamily::CNamed => ExcType::type_error_method_at_most(spec.func_name, max, n_pos + n_kw),
        ErrorFamily::Clinic => ExcType::type_error_at_most(spec.func_name, max, n_pos + n_kw),
    }
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
