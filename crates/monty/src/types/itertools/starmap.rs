//! `itertools.starmap(function, iterable)` — each item unpacked as the arguments.

use serde::{Deserialize, Serialize};

use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::{HeapRead, Reference, RuntimeRef},
    types::{
        iter::collect_owned_iterable,
        itertools::{ItertoolsIter, step::next_source},
    },
    value::Value,
};

#[derive(monty_macros::Relocate)]
/// Yields `function(*item)` for each item of `source`.
///
/// Every item must itself be iterable — `starmap(pow, [5])` raises `TypeError:
/// 'int' object is not iterable` mid-iteration, exactly as in CPython.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct StarMap<H: Reference> {
    function: Value<H>,
    source: Value<H>,
}

impl<H: RuntimeRef> StarMap<H> {
    /// Takes ownership of both, with `source` already resolved by `py_iter`.
    pub(crate) fn new(function: Value<H>, source: Value<H>) -> Self {
        Self { function, source }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(H)) {
        if let Value::Ref(id) = &self.function {
            on_child(*id);
        }
        if let Value::Ref(id) = &self.source {
            on_child(*id);
        }
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<H>) {
        self.function.py_dec_ref_ids(stack);
        self.source.py_dec_ref_ids(stack);
    }
}

/// Pulls one item, spreads it into an argument list, and calls the function.
pub(super) fn next<'h, H: RuntimeRef>(
    iter: &HeapRead<'h, ItertoolsIter<H>>,
    vm: &mut VM<'h, H>,
) -> RunResult<Option<Value<H>>> {
    let ItertoolsIter::StarMap(starmap) = iter.get(vm.heap) else {
        unreachable!("dispatched on Kind::StarMap")
    };
    let function = starmap.function.clone_with_heap(vm.heap);
    let source = starmap.source.clone_with_heap(vm.heap);
    defer_drop!(function, vm);
    defer_drop!(source, vm);

    let Some(item) = next_source(source, vm)? else {
        return Ok(None);
    };
    // `collect_owned_iterable` consumes `item` on both paths, raising here for
    // a non-iterable one, so the arguments never outlive a failed spread.
    let args: Vec<Value<H>> = collect_owned_iterable(item, vm)?;
    vm.evaluate_function("starmap()", function, pack_args(args)).map(Some)
}

/// Packs the spread item into the arity-specific [`ArgValues`] shape.
///
/// The small forms are not interchangeable with `ArgsKargs`: extractors such as
/// `ArgValues::get_one_arg` match `One` structurally, so a one-element
/// `ArgsKargs` is rejected as the wrong shape (`abs() takes exactly one
/// argument`) even though the arity is right.
fn pack_args<H: RuntimeRef>(items: Vec<Value<H>>) -> ArgValues<H> {
    match <[Value<H>; 1]>::try_from(items) {
        Ok([first]) => ArgValues::One(first),
        Err(items) => match <[Value<H>; 2]>::try_from(items) {
            Ok([first, second]) => ArgValues::Two(first, second),
            Err(items) if items.is_empty() => ArgValues::Empty,
            Err(items) => ArgValues::ArgsKargs {
                args: items,
                kwargs: KwargsValues::Empty,
            },
        },
    }
}
