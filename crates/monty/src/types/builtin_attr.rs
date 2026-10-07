//! Static attribute definitions for builtin types.

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::RunResult,
    heap::{HeapObjectRead, HeapRead},
    intern::StaticStrings,
    types::{
        Complex, List, ReMatch, RePattern, TimeDelta, TimeZone, Tuple, Type, complex, date, list, re_match, re_pattern,
        time, timedelta, timezone, tuple,
    },
    value::Value,
};

pub(crate) type BuiltinCall = for<'h> fn(StaticStrings, Type, Value, ArgValues, &mut VM<'h>) -> RunResult<CallResult>;

#[allow(dead_code)]
#[derive(Clone, Copy)]
pub(crate) enum AttrDef {
    Method(MethodDef),
    ClassMethod(BuiltinCall),
    Value(fn(&mut VM<'_>) -> Value),
}

pub(crate) type Method<T> = for<'h> fn(&mut HeapRead<'h, T>, ArgValues, &mut VM<'h>) -> RunResult<Value>;
pub(crate) type ReadMethod<T> = for<'h> fn(&HeapRead<'h, T>, ArgValues, &mut VM<'h>) -> RunResult<Value>;
pub(crate) type ObjectMethod<T> = for<'h> fn(&mut HeapObjectRead<'h, T>, ArgValues, &mut VM<'h>) -> RunResult<Value>;

#[allow(dead_code)]
#[derive(Clone, Copy)]
pub(crate) enum MethodDef {
    List(Method<List>),
    Tuple(ReadMethod<Tuple>),
    Complex(ObjectMethod<Complex>),
    Date(ObjectMethod<date::Date>),
    Time(ObjectMethod<time::Time>),
    TimeDelta(ObjectMethod<TimeDelta>),
    TimeZone(ObjectMethod<TimeZone>),
    RePattern(Method<RePattern>),
    ReMatch(Method<ReMatch>),
}

impl AttrDef {
    pub(crate) const fn method(call: MethodDef) -> Self {
        Self::Method(call)
    }

    pub(crate) const fn class_method(call: BuiltinCall) -> Self {
        Self::ClassMethod(call)
    }

    pub(crate) const fn value(get: fn(&mut VM<'_>) -> Value) -> Self {
        Self::Value(get)
    }
}

macro_rules! builtin_attrs {
    (@def method($handler:path)) => {
        attr_method($handler)
    };
    (@def class_method($handler:path)) => {
        $crate::types::builtin_attr::AttrDef::class_method($handler)
    };
    (@def value($handler:path)) => {
        $crate::types::builtin_attr::AttrDef::value($handler)
    };
    (
        $vis:vis const $table:ident: &[(StaticStrings, AttrDef)] = &[
            $($name:ident => $kind:ident($handler:path)),* $(,)?
        ];
        $lookup_vis:vis const fn $lookup:ident;
    ) => {
        #[allow(dead_code)]
        $vis const $table: &[(StaticStrings, AttrDef)] = &[
            $((StaticStrings::$name, $crate::types::builtin_attr::builtin_attrs!(@def $kind($handler))),)*
        ];

        #[allow(dead_code)]
        $lookup_vis const fn $lookup(name: StaticStrings) -> Option<AttrDef> {
            match name {
                $(StaticStrings::$name => Some($crate::types::builtin_attr::builtin_attrs!(@def $kind($handler))),)*
                _ => None,
            }
        }
    };
}

pub(crate) use builtin_attrs;

#[allow(dead_code)]
pub(crate) const fn attrs(owner: Type) -> &'static [(StaticStrings, AttrDef)] {
    match owner {
        Type::List => list::ATTRS,
        Type::Tuple => tuple::ATTRS,
        Type::Complex => complex::ATTRS,
        Type::Date => date::ATTRS,
        Type::Time => time::ATTRS,
        Type::TimeDelta => timedelta::ATTRS,
        Type::TimeZone => timezone::ATTRS,
        Type::RePattern => re_pattern::ATTRS,
        Type::ReMatch => re_match::ATTRS,
        _ => &[],
    }
}

#[allow(dead_code)]
pub(crate) const fn lookup_attr(owner: Type, name: StaticStrings) -> Option<AttrDef> {
    match owner {
        Type::List => list::lookup_attr(name),
        Type::Tuple => tuple::lookup_attr(name),
        Type::Complex => complex::lookup_attr(name),
        Type::Date => date::lookup_attr(name),
        Type::Time => time::lookup_attr(name),
        Type::TimeDelta => timedelta::lookup_attr(name),
        Type::TimeZone => timezone::lookup_attr(name),
        Type::RePattern => re_pattern::lookup_attr(name),
        Type::ReMatch => re_match::lookup_attr(name),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_and_lookups_agree() {
        for owner in [
            Type::List,
            Type::Tuple,
            Type::Complex,
            Type::Date,
            Type::Time,
            Type::TimeDelta,
            Type::TimeZone,
            Type::RePattern,
            Type::ReMatch,
        ] {
            for &(name, definition) in attrs(owner) {
                let found = lookup_attr(owner, name).expect("table entry has a lookup arm");
                assert_eq!(std::mem::discriminant(&definition), std::mem::discriminant(&found));
            }
        }
    }
}
