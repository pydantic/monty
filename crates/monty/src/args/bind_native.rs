//! Signature checks for native Python callables.

use std::{array, mem};

use crate::{
    args::{ArgPosIter, ArgValues, KwargsValues},
    bytecode::VM,
    defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{ContainsHeap, DropWithContext, Heap},
    intern::StaticStrings,
    value::Value,
};

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
    accepts_keywords: bool,
    kwargs_not_supported_yet: bool,
}

impl<const N: usize> ParamSpec<N> {
    pub(crate) const fn new(params: [Param; N]) -> Self {
        let mut positional = 0;
        let mut accepts_keywords = false;
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
            accepts_keywords |= !matches!(params[i].kind, ParamKind::PosOnly(_));
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
            accepts_keywords,
            kwargs_not_supported_yet: false,
        }
    }

    pub(super) const fn positional_only(required: usize) -> Self {
        assert!(required <= N);
        let mut params = [const { Param::pos_only("").optional() }; N];
        let mut i = 0;
        while i < required {
            params[i].required = true;
            i += 1;
        }
        Self::new(params)
    }

    pub(crate) const fn varargs(mut self, name: &'static str) -> Self {
        self.varargs = Some(name);
        self
    }

    pub(crate) const fn varkwargs(mut self, name: &'static str) -> Self {
        self.varkwargs = Some(name);
        self.accepts_keywords = true;
        self
    }

    pub(crate) const fn kwargs_not_supported_yet(mut self) -> Self {
        self.kwargs_not_supported_yet = true;
        self
    }

    #[cold]
    fn missing_arguments(&self, name: &str, is_missing: impl Fn(usize) -> bool) -> RunError {
        let positional: Vec<_> = self.params[..self.positional]
            .iter()
            .enumerate()
            .filter(|(i, param)| param.required && is_missing(*i))
            .map(|(_, param)| param.name())
            .collect();
        if !positional.is_empty() {
            return ExcType::type_error_missing_positional_with_names(name, &positional);
        }
        let keyword_only: Vec<_> = self.params[self.positional..]
            .iter()
            .enumerate()
            .filter(|(i, param)| param.required && is_missing(self.positional + i))
            .map(|(_, param)| param.name())
            .collect();
        ExcType::type_error_missing_kwonly_with_names(name, &keyword_only)
    }

    #[cold]
    fn positional_arity_error(&self, name: &str, actual: usize) -> RunError {
        let minimum = self.required_prefix;
        let maximum = self.positional;
        if self.varargs.is_none() && minimum == maximum {
            if maximum == 0 {
                return ExcType::type_error(format_args!("{name}() takes no arguments ({actual} given)"));
            }
            let plural = if maximum == 1 { "" } else { "s" };
            return ExcType::type_error(format_args!(
                "{name}() takes exactly {maximum} argument{plural} ({actual} given)"
            ));
        }
        if actual < minimum {
            let plural = if minimum == 1 { "" } else { "s" };
            return ExcType::type_error(format_args!(
                "{name}() takes at least {minimum} argument{plural} ({actual} given)"
            ));
        }
        self.positional_overflow(name, actual)
    }

    #[cold]
    fn positional_overflow(&self, name: &str, actual: usize) -> RunError {
        let positional = if self.varkwargs.is_some() || self.positional < N {
            "positional "
        } else {
            ""
        };
        let plural = if self.positional == 1 { "" } else { "s" };
        ExcType::type_error(format_args!(
            "{name}() takes at most {} {positional}argument{plural} ({actual} given)",
            self.positional
        ))
    }

    #[cold]
    fn invalid_keyword(&self, name: &str, key: &str) -> RunError {
        if self
            .params
            .iter()
            .any(|param| matches!(param.kind, ParamKind::PosOnly(param_name) if param_name == key))
        {
            ExcType::type_error_positional_only(name, key)
        } else {
            ExcType::type_error_unexpected_keyword(name, key)
        }
    }

    fn accepts_positional_count(&self, count: usize) -> bool {
        self.required_prefix <= count && count <= self.positional
    }

    /// Unpacks a fixed list of positional-only parameters.
    pub(super) fn unpack_positional(
        &self,
        name: &str,
        args: ArgValues,
        heap: &mut Heap,
    ) -> RunResult<[Option<Value>; N]> {
        let mut values = args.into_pos_only(name, heap)?;
        let count = values.len();
        if !self.accepts_positional_count(count) {
            values.drop_with(heap);
            return Err(self.positional_arity_error(name, count));
        }
        Ok(array::from_fn(|_| values.next()))
    }

    /// The caller's guard owns all arguments transferred into `bound`.
    #[inline]
    pub(crate) fn bind(&self, name: &str, bound: &mut Bound<N>, args: ArgValues, vm: &mut VM<'_>) -> RunResult<()> {
        match args {
            ArgValues::Empty if self.accepts_positional_count(0) => Ok(()),
            ArgValues::One(v) if self.accepts_positional_count(1) => {
                bound.slots[0] = Some(v);
                Ok(())
            }
            ArgValues::Two(a, b) if self.accepts_positional_count(2) => {
                bound.slots[0] = Some(a);
                bound.slots[1] = Some(b);
                Ok(())
            }
            other => self.bind_slow(name, bound, other, vm),
        }
    }

    fn bind_slow(&self, name: &str, bound: &mut Bound<N>, args: ArgValues, vm: &mut VM<'_>) -> RunResult<()> {
        let (pos, kwargs) = args.into_parts();
        let remaining = (pos, kwargs.into_iter());
        defer_drop_mut!(remaining, vm);
        let (pos, kwargs) = remaining;
        if self.kwargs_not_supported_yet && kwargs.len() > 0 {
            return Err(ExcType::kwargs_not_implemented(name));
        }
        let positional_count = pos.len();
        for slot in bound.slots.iter_mut().take(positional_count.min(self.positional)) {
            *slot = pos.next();
        }
        if self.varargs.is_some() {
            bound.varargs = match mem::replace(pos, ArgPosIter::Empty) {
                ArgPosIter::Empty => Vec::new(),
                ArgPosIter::One(value) => vec![value],
                ArgPosIter::Two(values) => Vec::from(values),
                ArgPosIter::Vec(values) => values.collect(),
            };
        }

        for (key, value) in kwargs.by_ref() {
            let Some(key_str) = key.as_str(vm) else {
                (key, value).drop_with(vm);
                return Err(ExcType::type_error_kwargs_nonstring_key());
            };
            if !self.accepts_keywords {
                (key, value).drop_with(vm);
                return Err(ExcType::type_error_no_kwargs(name));
            }

            let tag = match &key {
                Value::InternString(id) => vm.interns.static_string(*id),
                _ => key_str.parse().ok(),
            };
            let slot = tag.and_then(|tag| self.params.iter().position(|param| param.keyword_name() == Some(tag)));
            match slot {
                Some(i) if bound.slots[i].is_none() => {
                    key.drop_with(vm);
                    bound.slots[i] = Some(value);
                }
                None if self.varkwargs.is_some() => bound.varkwargs.push((key, value)),
                Some(i) => {
                    (key, value).drop_with(vm);
                    return Err(ExcType::type_error_duplicate_arg(name, self.params[i].name()));
                }
                None => {
                    let error = self.invalid_keyword(name, key_str);
                    (key, value).drop_with(vm);
                    return Err(error);
                }
            }
        }
        if positional_count > self.positional && self.varargs.is_none() {
            return Err(if self.accepts_keywords {
                self.positional_overflow(name, positional_count)
            } else {
                self.positional_arity_error(name, positional_count)
            });
        }
        if self
            .params
            .iter()
            .zip(&bound.slots)
            .any(|(param, slot)| param.required && slot.is_none())
        {
            return Err(if self.accepts_keywords {
                self.missing_arguments(name, |i| bound.slots[i].is_none())
            } else {
                self.positional_arity_error(name, positional_count)
            });
        }
        Ok(())
    }
}

pub(crate) struct Bound<const N: usize> {
    slots: [Option<Value>; N],
    varargs: Vec<Value>,
    varkwargs: Vec<(Value, Value)>,
}

impl<const N: usize> Bound<N> {
    pub(crate) fn new() -> Self {
        Self {
            slots: [const { None }; N],
            varargs: Vec::new(),
            varkwargs: Vec::new(),
        }
    }

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

#[cfg(test)]
mod tests {
    use super::{Param, ParamSpec, StaticStrings};

    #[test]
    fn required_keyword_only_parameter_excludes_positional_fast_paths() {
        const SPEC: ParamSpec<2> =
            ParamSpec::new([Param::pos_only("x").optional(), Param::kw_only(StaticStrings::Key)]);
        for count in 0..=2 {
            assert!(!SPEC.accepts_positional_count(count));
        }
    }
}
