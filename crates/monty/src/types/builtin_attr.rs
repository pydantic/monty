//! Static attribute definitions for builtin types.

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::RunResult,
    intern::StaticStrings,
    types::{Type, complex, date, list, re_match, re_pattern, time, timedelta, timezone, tuple},
    value::Value,
};

pub(crate) type BuiltinCall = for<'h> fn(StaticStrings, Type, Value, ArgValues, &mut VM<'h>) -> RunResult<CallResult>;
pub(crate) type MethodCall = for<'h> fn(&Value, ArgValues, &mut VM<'h>) -> RunResult<Value>;

#[derive(Clone, Copy)]
pub(crate) enum AttrDef {
    Method(MethodCall),
    ClassMethod(BuiltinCall),
    Value(fn(&mut VM<'_>) -> Value),
}

impl AttrDef {
    pub(crate) const fn method(call: MethodCall) -> Self {
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
    (@def immediate $variant:ident, method($handler:path)) => {
        $crate::types::builtin_attr::AttrDef::method({
            fn call(receiver: &$crate::value::Value, args: $crate::args::ArgValues, vm: &mut $crate::bytecode::VM<'_>) -> $crate::exception_private::RunResult<$crate::value::Value> {
                let $crate::value::Value::$variant(value) = receiver else {
                    unreachable!("builtin method receiver must match its attribute table")
                };
                $handler(*value, args, vm)
            }
            call
        })
    };
    (@def mut_heap $variant:ident, method($handler:path)) => {
        $crate::types::builtin_attr::AttrDef::method({
            fn call(receiver: &$crate::value::Value, args: $crate::args::ArgValues, vm: &mut $crate::bytecode::VM<'_>) -> $crate::exception_private::RunResult<$crate::value::Value> {
                let $crate::value::Value::Ref(id) = receiver else {
                    unreachable!("builtin method receiver must be a heap value")
                };
                let $crate::heap::HeapReadOutput::$variant(mut value) = vm.heap.read(*id) else {
                    unreachable!("builtin method receiver must match its attribute table")
                };
                $handler(&mut value, args, vm)
            }
            call
        })
    };
    (@def heap $variant:ident, method($handler:path)) => {
        $crate::types::builtin_attr::AttrDef::method({
            fn call(receiver: &$crate::value::Value, args: $crate::args::ArgValues, vm: &mut $crate::bytecode::VM<'_>) -> $crate::exception_private::RunResult<$crate::value::Value> {
                let $crate::value::Value::Ref(id) = receiver else {
                    unreachable!("builtin method receiver must be a heap value")
                };
                let $crate::heap::HeapReadOutput::$variant(value) = vm.heap.read(*id) else {
                    unreachable!("builtin method receiver must match its attribute table")
                };
                $handler(&value, args, vm)
            }
            call
        })
    };
    (@def $mode:ident $variant:ident, class_method($handler:path)) => {
        $crate::types::builtin_attr::AttrDef::class_method($handler)
    };
    (@def $mode:ident $variant:ident, value($handler:path)) => {
        $crate::types::builtin_attr::AttrDef::value($handler)
    };
    (
        for $owner:ident: mut heap($variant:ident);
        $($rest:tt)*
    ) => {
        $crate::types::builtin_attr::builtin_attrs!(@table mut_heap $variant; $($rest)*);
    };
    (
        for $owner:ident: heap($variant:ident);
        $($rest:tt)*
    ) => {
        $crate::types::builtin_attr::builtin_attrs!(@table heap $variant; $($rest)*);
    };
    (
        for $owner:ident: immediate($variant:ident);
        $($rest:tt)*
    ) => {
        $crate::types::builtin_attr::builtin_attrs!(@table immediate $variant; $($rest)*);
    };
    (
        @table $mode:ident $variant:ident;
        $vis:vis const $table:ident: &[(StaticStrings, AttrDef)] = &[
            $($name:ident => $kind:ident($handler:path)),* $(,)?
        ];
        $lookup_vis:vis const fn $lookup:ident;
    ) => {
        $vis const $table: &[(StaticStrings, AttrDef)] = &[
            $((StaticStrings::$name, $crate::types::builtin_attr::builtin_attrs!(@def $mode $variant, $kind($handler))),)*
        ];

        $lookup_vis const fn $lookup(name: StaticStrings) -> Option<AttrDef> {
            match name {
                $(StaticStrings::$name => Some($crate::types::builtin_attr::builtin_attrs!(@def $mode $variant, $kind($handler))),)*
                _ => None,
            }
        }
    };
}

pub(crate) use builtin_attrs;

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
