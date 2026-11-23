//! Lazy builtin adaptors with native fast paths and suspendable per-item frames.

use std::{fmt::Write, mem};

use serde::{Deserialize, Serialize};

use crate::{
    args::{ArgValues, FromArgs},
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::{CallResult, RunReentryGuard, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    frozen::{FrozenFunction, FrozenFunctions},
    hash::{HashValue, identity_hash},
    heap::{DropGuard, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput},
    modules::ModuleFunctions,
    py_value::PyValue,
    types::{LazyHeapSet, List, PyTrait, Type, TypeRef},
    value::Value,
};

/// Persistent state between `next()` calls; in-flight work belongs to ordinary VM frames.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum MapFilter {
    /// A map owns its callback and a private list of source iterators.
    Map {
        function: Value,
        iterators: Value,
        strict: bool,
    },
    /// A filter owns its predicate (possibly `None`) and source iterator.
    Filter { function: Value, iterator: Value },
}

impl MapFilter {
    /// Acquires native sources directly; user protocols retain their suspendable construction frame.
    pub(crate) fn new_map(
        function: Value,
        iterables: Vec<Value>,
        strict: bool,
        vm: &mut VM<'_>,
    ) -> RunResult<CallResult> {
        if iterables.iter().all(|source| has_native_iter(source, vm)) {
            let mut inputs = DropGuard::new((function, iterables), vm);
            let ((_, sources), vm) = inputs.as_parts_mut();
            for source in sources {
                *source = mem::replace(source, Value::Undefined)
                    .into_py_iter(vm)?
                    .require_synchronous(vm)?;
            }
            let ((function, iterators), vm) = inputs.into_parts();
            let iterators = Value::Ref(vm.heap.allocate(HeapData::List(List::new(iterators))));
            Ok(CallResult::Value(
                Self::Map {
                    function,
                    iterators,
                    strict,
                }
                .allocate(vm),
            ))
        } else {
            let iterables = Value::Ref(vm.heap.allocate(HeapData::List(List::new(iterables))));
            vm.call_frozen_exact(
                FrozenFunction::MapInit,
                [
                    function,
                    iterables,
                    Value::Bool(strict),
                    Value::Builtin(Builtins::Type(Type::Iterator)),
                    Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::BuildMap)),
                ],
            )
        }
    }

    /// Acquires a builtin iterable without compiling a constructor continuation.
    pub(crate) fn new_filter(function: Value, iterable: Value, vm: &mut VM<'_>) -> RunResult<CallResult> {
        if has_native_iter(&iterable, vm) {
            let mut function = DropGuard::new(function, vm);
            let iterator = iterable
                .into_py_iter(function.ctx())?
                .require_synchronous(function.ctx())?;
            let (function, vm) = function.into_parts();
            Ok(CallResult::Value(Self::Filter { function, iterator }.allocate(vm)))
        } else {
            vm.call_frozen_exact(
                FrozenFunction::FilterInit,
                [
                    function,
                    iterable,
                    Value::Builtin(Builtins::Type(Type::Iterator)),
                    Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::BuildFilter)),
                ],
            )
        }
    }

    /// Transfers a fully acquired adaptor into the heap.
    fn allocate(self, vm: &VM<'_>) -> Value {
        Value::Ref(vm.heap.allocate(HeapData::MapFilter(Box::new(self))))
    }

    /// Returns the public builtin type, independent of the shared storage representation.
    pub(crate) fn py_type(&self) -> Type {
        match self {
            Self::Map { .. } => Type::Map,
            Self::Filter { .. } => Type::Filter,
        }
    }

    /// Enumerates every owned heap reference, including cycles through a callback or source.
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        let (function, source) = match self {
            Self::Map {
                function, iterators, ..
            } => (function, iterators),
            Self::Filter { function, iterator } => (function, iterator),
        };
        for value in [function, source] {
            if let Value::Ref(id) = value {
                on_child(*id);
            }
        }
    }
}

impl HeapItem for MapFilter {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        let (function, source) = match self {
            Self::Map {
                function, iterators, ..
            } => (function, iterators),
            Self::Filter { function, iterator } => (function, iterator),
        };
        function.py_dec_ref_ids(stack);
        source.py_dec_ref_ids(stack);
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, MapFilter> {
    fn py_type(&self, vm: &VM<'h>) -> TypeRef {
        self.get(vm.heap).py_type().into()
    }

    fn py_hash(&self, _: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_is_iterable(&self, _: &VM<'h>) -> bool {
        true
    }

    fn py_is_iterator(&self, _: &VM<'h>) -> bool {
        true
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        Ok(CallResult::Value(self.clone_value(vm.heap)))
    }

    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        // Copy owned inputs before calling Python: callbacks may re-enter this same iterator.
        match self.get(vm.heap) {
            MapFilter::Map {
                function,
                iterators,
                strict,
            } => {
                let Some(HeapReadOutput::List(sources)) = iterators.read_heap(vm) else {
                    unreachable!("map owns a private list of source iterators")
                };
                let sources = sources.get(vm.heap).items();
                if sources.len() == 1
                    && let Some(mut iterator) = sources.map_item(0, |source| PyValue::native_iterator(source, vm))
                {
                    let function = function.clone_with_heap(vm);
                    defer_drop!(function, vm);
                    let Some(item) = iterator.next_sync(vm)? else {
                        return Ok(None);
                    };
                    // A builtin callback can re-enter map without pushing a frame. Charge dispatch only:
                    // a pushed frame or host call will resume after the native stack has unwound.
                    let mut item = DropGuard::new(item, vm);
                    item.ctx().enter_run_reentry()?;
                    let (item, vm) = item.into_parts();
                    return RunReentryGuard::new(vm)
                        .call_function(function, ArgValues::One(item))
                        .map(Some);
                }
                let args = [
                    function.clone_with_heap(vm),
                    iterators.clone_with_heap(vm),
                    Value::Bool(*strict),
                    Value::Builtin(Builtins::Function(BuiltinsFunctions::Next)),
                    Value::Builtin(Builtins::Type(Type::StopIteration)),
                    Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::MapLengthError)),
                ];
                vm.call_frozen_exact(FrozenFunction::MapNext, args).map(Some)
            }
            MapFilter::Filter { function, iterator } => {
                if matches!(function, Value::None)
                    && let Some(mut source) = PyValue::native_iterator(iterator, vm)
                {
                    return filter_truthy(&mut source, vm).map(|item| item.map(CallResult::Value));
                }
                let args = [
                    function.clone_with_heap(vm),
                    iterator.clone_with_heap(vm),
                    Value::Builtin(Builtins::Function(BuiltinsFunctions::Next)),
                ];
                vm.call_frozen_exact(FrozenFunction::FilterNext, args).map(Some)
            }
        }
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _: &mut LazyHeapSet) -> RunResult<()> {
        self.py_default_repr_fmt(f, vm)
    }
}

/// Builtin containers and iterator identities acquire without calling user protocols.
fn has_native_iter(value: &Value, vm: &VM<'_>) -> bool {
    let Some(ty) = value.py_type(vm).native() else {
        return false;
    };
    ty.is_iterator()
        || matches!(
            ty,
            Type::List
                | Type::Tuple
                | Type::NamedTuple
                | Type::Range
                | Type::Str
                | Type::Bytes
                | Type::Dict
                | Type::Set
                | Type::FrozenSet
                | Type::Deque
        )
}

/// Advances a non-recursive native source until it yields a truthy item.
/// Poll limits even when an endlessly rejecting native source executes no bytecode.
fn filter_truthy<'h>(iterator: &mut PyValue<'h>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    let mut seen = 0;
    while let Some(item) = iterator.next_sync(vm)? {
        let mut item = DropGuard::new(item, vm);
        let (value, vm) = item.as_parts_mut();
        seen += 1;
        vm.heap.tracker.check_memory_time_every(seen)?;
        if value.py_bool(vm)? {
            return Ok(Some(item.into_inner()));
        }
    }
    Ok(None)
}

/// Acquires each source in order without consuming items or calling the mapped function.
pub(crate) const MAP_INIT_SOURCE: &str = r"
def __monty_map_init(function, iterables, strict, iter_, build):
    iterators = []
    for iterable in iterables:
        iterators.append(iter_(iterable))
    return build(function, iterators, strict)
";

/// Acquires a filter's source without evaluating its predicate.
pub(crate) const FILTER_INIT_SOURCE: &str = r"
def __monty_filter_init(function, iterable, iter_, build):
    return build(function, iter_(iterable))
";

/// One mapped item; strict exhaustion probes happen in source order, never around the callback.
pub(crate) const MAP_NEXT_SOURCE: &str = r"
def __monty_map_next(function, iterators, strict, next_, stop_iteration, length_error):
    items = []
    index = 0
    for iterator in iterators:
        try:
            item = next_(iterator)
        except stop_iteration:
            if strict:
                if index:
                    length_error(index, False)
                probe_index = 0
                for other in iterators:
                    if probe_index:
                        try:
                            next_(other)
                        except stop_iteration:
                            pass
                        else:
                            length_error(probe_index, True)
                    probe_index = probe_index + 1
            raise
        items.append(item)
        index = index + 1
    return function(*items)
";

/// Returns one accepted item; no frame is retained between successful `next()` calls.
pub(crate) const FILTER_NEXT_SOURCE: &str = r"
def __monty_filter_next(function, iterator, next_):
    while True:
        item = next_(iterator)
        if function is None:
            if item:
                return item
        elif function(item):
            return item
";

/// Private map builder arguments, after the frozen constructor has acquired every iterator.
#[derive(FromArgs)]
#[from_args(name = "build_map")]
struct BuildMapArgs {
    #[from_args(pos_only)]
    function: Value,
    #[from_args(pos_only)]
    iterators: Value,
    #[from_args(pos_only)]
    strict: bool,
}

/// Transfers already-acquired sources into a new map object.
pub(crate) fn build_map(args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let BuildMapArgs {
        function,
        iterators,
        strict,
    } = BuildMapArgs::from_args(args, vm)?;
    Ok(MapFilter::Map {
        function,
        iterators,
        strict,
    }
    .allocate(vm))
}

/// Transfers an acquired source into a new filter object.
pub(crate) fn build_filter(args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let (function, iterator) = args.get_two_args("build_filter", vm.heap)?;
    Ok(MapFilter::Filter { function, iterator }.allocate(vm))
}

/// Raises the exact strict-map mismatch for a zero-based source index.
pub(crate) fn map_length_error(args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let (index, longer) = args.get_two_args("map_length_error", vm.heap)?;
    defer_drop!(index, vm);
    defer_drop!(longer, vm);
    let Value::Int(index) = index else {
        unreachable!("frozen map counts sources with integers")
    };
    let relation = if matches!(longer, Value::Bool(true)) {
        "longer"
    } else {
        "shorter"
    };
    let others = if *index == 1 {
        "argument 1".to_owned()
    } else {
        format!("arguments 1-{index}")
    };
    Err(ExcType::value_error(format!(
        "map() argument {} is {relation} than {others}",
        index + 1
    )))
}
