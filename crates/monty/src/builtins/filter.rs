//! Argument binding for the lazy filter iterator.

use monty_macros::py_signature;

use crate::{
    bytecode::{CallResult, VM},
    exception_private::RunResult,
    types::map_filter::MapFilter,
};

/// Acquires the source iterator at construction, leaving predicate calls until advancement.
#[py_signature("filter(function, iterable, /)")]
pub fn builtin_filter(vm: &mut VM<'_>, params: &mut FilterArgs) -> RunResult<CallResult> {
    MapFilter::new_filter(params.function.take(), params.iterable.take(), vm)
}
