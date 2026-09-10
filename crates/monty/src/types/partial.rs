//! `functools.partial` — a callable that binds leading arguments to another callable.
//!
//! Calling one is dispatched by `VM::call_heap_callable`, which reads the bound
//! arguments out of the heap, merges the call's own on top, and re-enters
//! `call_function` with the wrapped callable. Nothing here calls Python, so a
//! `partial` wrapping an external function still suspends to the host normally.

use std::{
    fmt::Write,
    iter::once,
    mem::{replace, take},
};

use ahash::AHashMap;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::{
    args::{ArgValues, KwargsValues},
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, identity_hash},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead},
    intern::StaticStrings,
    types::{Dict, LazyHeapSet, PyTrait, Type, list::repr_check_time, tuple::allocate_tuple},
    value::{EitherStr, VALUE_SIZE, Value},
};

/// A callable that prepends bound arguments to `func` on every call.
///
/// `func`, every value in `args`, and both halves of every `keywords` pair are
/// OWNED refs held directly here rather than in a container this points at, so
/// `py_dec_ref_ids` and `for_each_child_id` must enumerate all of them or the
/// GC will under-trace and free a live object.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Partial {
    /// The wrapped callable. Never itself a `partial`: [`Partial::init`]
    /// flattens those at construction, as CPython does.
    func: Value,
    /// Positional arguments passed before the call's own.
    args: Vec<Value>,
    /// Bound keywords as `(name, value)` pairs, names being `str` values.
    ///
    /// A `Vec` rather than a `Dict` because merging has to preserve the bound
    /// order while letting call-time keywords replace values in place, and
    /// there are rarely more than a couple of names to scan.
    keywords: Vec<(Value, Value)>,
}

/// The callable, bound positionals and bound keywords lifted out of a partial,
/// as owned clones (see [`Partial::clone_parts`]).
type PartialParts = (Value, Vec<Value>, Vec<(Value, Value)>);

impl Partial {
    /// `functools.partial(func, /, *args, **keywords)`.
    ///
    /// A `func` that is itself a `partial` is flattened: its callable, bound
    /// arguments and keywords are merged into the new one, so
    /// `partial(partial(f, 1), 2)` is `partial(f, 1, 2)` and reprs as such.
    pub(crate) fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        // CPython's `partial_new` checks the positional count before anything
        // else, so `partial(func=f)` reports a missing argument rather than an
        // unexpected keyword.
        if args.count() == 0 {
            args.drop_with(vm);
            return Err(ExcType::partial_needs_argument());
        }

        let (positional, keywords) = args.into_parts();
        let positional = positional.collect::<Vec<_>>();
        defer_drop_mut!(positional, vm);
        let keywords = keywords.into_iter().collect::<Vec<_>>();
        defer_drop_mut!(keywords, vm);

        let func = positional.remove(0);
        if !func.is_callable(vm.heap) {
            func.drop_with(vm);
            return Err(ExcType::partial_not_callable());
        }

        let mut guard = DropGuard::new(
            Self {
                func,
                args: take(positional),
                keywords: Vec::new(),
            },
            vm,
        );
        let (partial, vm) = guard.as_parts_mut();
        // Flattening clones the inner partial's contents, so it can fail on the
        // allocation preflight; the guard then releases the half-built partial.
        partial.flatten(vm)?;
        // Bound keywords are merged the same way a call's are, so a flattened
        // inner keyword is replaced rather than duplicated.
        merge_keywords(&mut partial.keywords, keywords.drain(..), vm);

        let (partial, vm) = guard.into_parts();
        Ok(Value::Ref(vm.heap.allocate(HeapData::Partial(Box::new(partial)))))
    }

    /// Invokes `on_child` for each heap id this partial owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        for value in self.owned_values() {
            if let Value::Ref(id) = value {
                on_child(*id);
            }
        }
    }

    /// Every owned `Value`, in no particular order.
    fn owned_values(&self) -> impl Iterator<Item = &Value> {
        once(&self.func)
            .chain(self.args.iter())
            .chain(self.keywords.iter().flat_map(|(name, value)| [name, value]))
    }

    /// Splices a `partial` wrapped by this one into it, leaving `func` pointing
    /// at the innermost callable.
    ///
    /// Only callable before any keyword is bound: the inner partial's keywords
    /// replace `self.keywords` wholesale, so anything already there would leak.
    /// CPython only unwraps an exact `partial`, so a subclass instance (which
    /// Monty cannot create anyway) would stay nested.
    fn flatten(&mut self, vm: &mut VM<'_>) -> RunResult<()> {
        debug_assert!(
            self.keywords.is_empty(),
            "flatten() overwrites keywords rather than merging"
        );
        let Value::Ref(inner_id) = self.func else { return Ok(()) };
        let HeapData::Partial(inner) = vm.heap.get(inner_id) else {
            return Ok(());
        };
        let (func, mut args, keywords) = inner.clone_parts(vm)?;

        args.append(&mut self.args);
        let outer_func = replace(&mut self.func, func);
        outer_func.drop_with(vm);
        self.args = args;
        self.keywords = keywords;
        Ok(())
    }

    /// Owned clones of the callable, bound positionals and bound keywords.
    ///
    /// Takes the heap by shared reference (`clone_with_heap` only needs
    /// `inc_ref`), so a caller holding this partial through a `&HeapData` can
    /// lift its contents out and then release the borrow.
    ///
    /// Preflights both vectors through [`check_clone_slots`].
    pub(crate) fn clone_parts(&self, heap: &impl ContainsHeap) -> RunResult<PartialParts> {
        check_clone_slots(
            self.args.len().saturating_add(self.keywords.len().saturating_mul(2)),
            heap,
        )?;
        Ok((
            self.func.clone_with_heap(heap),
            self.args.iter().map(|arg| arg.clone_with_heap(heap)).collect(),
            self.keywords
                .iter()
                .map(|(name, value)| (name.clone_with_heap(heap), value.clone_with_heap(heap)))
                .collect(),
        ))
    }
}

/// Preflights the bytes a bulk clone of `slots` values will allocate.
///
/// Every path that lifts a partial's contents out — a call, `p.args`,
/// `p.keywords` — rebuilds them in full, so each needs the up-front check the
/// container clones use (`List::clone_all_items`, `Dict::clone_all_pairs`);
/// without it a widely bound partial bursts past the allocator's hard limit
/// and kills the worker instead of raising `MemoryError`.
fn check_clone_slots(slots: usize, heap: &impl ContainsHeap) -> RunResult<()> {
    Ok(heap.heap().tracker.check_allocation(slots.saturating_mul(VALUE_SIZE))?)
}

/// Merges a call's `args` beneath the arguments a `partial` has bound.
///
/// The bound halves are already owned clones (see [`Partial::clone_parts`]), so
/// the heap borrow on the partial has ended by the time this runs and the
/// resulting call may re-enter that same partial.
pub(crate) fn partial_call_args(
    bound_args: Vec<Value>,
    bound_keywords: Vec<(Value, Value)>,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> ArgValues {
    let (positional, keywords) = args.into_parts();
    let mut args = bound_args;
    args.extend(positional);

    let mut merged = bound_keywords;
    merge_keywords(&mut merged, keywords.into_iter(), vm);
    let kwargs = if merged.is_empty() {
        KwargsValues::Empty
    } else {
        KwargsValues::Pairs(merged)
    };
    ArgValues::from_parts(args, kwargs)
}

/// Bound-keyword count above which merging switches from rescanning to a name
/// index. Below it the scan beats a map plus an owned key per incoming name.
const KEYWORD_INDEX_THRESHOLD: usize = 8;

/// Adds `keywords` on top of `bound`, replacing values in place.
///
/// An existing name keeps its position and loses its value, matching the
/// `PyDict_Merge` onto a copy of the bound dict that CPython performs. Takes
/// ownership of both halves of every pair, dropping whatever it displaces.
///
/// The scan is linear, so the whole merge is quadratic in the keyword count —
/// fine for the handful a partial normally carries, but the count is caller
/// controlled (`partial(f, **thousands)`) and nothing here reaches an execution
/// checkpoint. Past [`KEYWORD_INDEX_THRESHOLD`] it therefore builds a name
/// index and keeps it current, which the pushes below rely on.
fn merge_keywords(bound: &mut Vec<(Value, Value)>, keywords: impl Iterator<Item = (Value, Value)>, vm: &mut VM<'_>) {
    // Built lazily rather than up front because `bound` usually starts empty
    // and grows past the threshold during this very loop.
    let mut index: Option<AHashMap<String, usize>> = None;
    for (name, value) in keywords {
        if index.is_none() && bound.len() > KEYWORD_INDEX_THRESHOLD {
            index = Some(keyword_index_map(bound, vm));
        }
        let position = match &index {
            Some(index) => name
                .to_str_heap(vm.heap, vm.interns)
                .ok()
                .and_then(|name| index.get(name).copied()),
            None => keyword_index(bound, &name, vm),
        };
        if let Some(position) = position {
            let (old_name, old_value) = replace(&mut bound[position], (name, value));
            old_name.drop_with(vm);
            old_value.drop_with(vm);
        } else {
            if let Some(index) = &mut index
                && let Ok(name) = name.to_str_heap(vm.heap, vm.interns)
            {
                index.insert(name.to_owned(), bound.len());
            }
            bound.push((name, value));
        }
    }
}

/// Position of `name` among the `bound` keywords.
///
/// A non-`str` name never matches; the binder rejects it when the merged
/// arguments reach the wrapped callable.
fn keyword_index(bound: &[(Value, Value)], name: &Value, vm: &VM<'_>) -> Option<usize> {
    let name = name.to_str_heap(vm.heap, vm.interns).ok()?;
    bound
        .iter()
        .position(|(bound, _)| bound.to_str_heap(vm.heap, vm.interns).is_ok_and(|bound| bound == name))
}

/// Maps each bound keyword name to its position, for [`merge_keywords`].
///
/// Skips non-`str` names for the same reason [`keyword_index`] never matches
/// them. Names are owned copies so the map outlives the heap borrow that read
/// them, leaving the merge loop free to drop displaced values.
fn keyword_index_map(bound: &[(Value, Value)], vm: &VM<'_>) -> AHashMap<String, usize> {
    bound
        .iter()
        .enumerate()
        .filter_map(|(position, (name, _))| Some((name.to_str_heap(vm.heap, vm.interns).ok()?.to_owned(), position)))
        .collect()
}

/// Releases the refs a partial owns, for one abandoned before it reaches the
/// heap; a heap-stored partial is freed through [`HeapItem::py_dec_ref_ids`].
impl<C: ContainsHeap> DropWithContext<C> for Partial {
    fn drop_with(self, ctx: &mut C) {
        self.func.drop_with(ctx);
        self.args.drop_with(ctx);
        self.keywords.drop_with(ctx);
    }
}

impl HeapItem for Partial {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.func.py_dec_ref_ids(stack);
        for arg in &mut self.args {
            arg.py_dec_ref_ids(stack);
        }
        for (name, value) in &mut self.keywords {
            name.py_dec_ref_ids(stack);
            value.py_dec_ref_ids(stack);
        }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, Partial> {
    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::Partial
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    /// Partials hash by identity, as CPython's do — it defines neither
    /// `__eq__` nor `__hash__`, so two equivalent partials stay distinct keys.
    fn py_hash(&self, _: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    /// `functools.partial(<function f at 0x…>, 1, b=2)`.
    ///
    /// Takes a recursion level like the container reprs: a bound argument can
    /// hold a list that holds this partial, and that cycle prints as `...`.
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let Ok(mut guard) = vm.recursion_guard() else {
            return Ok(f.write_str("...")?);
        };
        let vm = &mut *guard;

        f.write_str("functools.partial(")?;
        let func = self.get(vm.heap).func.clone_with_heap(vm);
        defer_drop!(func, vm);
        func.py_repr_fmt(f, vm, heap_ids)?;

        // Both loops share one counter so the 64-item poll cadence spans the
        // whole partial, as `repr_items_fmt` does for a sequence: a widely
        // bound partial must not outrun `max_duration` between checkpoints.
        let mut item = 0;
        'items: {
            for index in 0..self.get(vm.heap).args.len() {
                if repr_check_time(item, vm) {
                    f.write_str(", ...[timeout]")?;
                    break 'items;
                }
                item += 1;
                let arg = self.get(vm.heap).args[index].clone_with_heap(vm);
                defer_drop!(arg, vm);
                f.write_str(", ")?;
                arg.py_repr_fmt(f, vm, heap_ids)?;
            }

            for index in 0..self.get(vm.heap).keywords.len() {
                if repr_check_time(item, vm) {
                    f.write_str(", ...[timeout]")?;
                    break 'items;
                }
                item += 1;
                let (name, value) = &self.get(vm.heap).keywords[index];
                let (name, value) = (name.clone_with_heap(vm), value.clone_with_heap(vm));
                defer_drop!(name, vm);
                defer_drop!(value, vm);
                // CPython writes the name unquoted, as keyword syntax rather than
                // as a dict key. The `repr` fallback is for a non-`str` name, which
                // Monty rejects at the `**` unpack before it can reach a partial.
                if let Ok(name) = name.to_str_heap(vm.heap, vm.interns) {
                    write!(f, ", {name}=")?;
                } else {
                    f.write_str(", ")?;
                    name.py_repr_fmt(f, vm, heap_ids)?;
                    f.write_str("=")?;
                }
                value.py_repr_fmt(f, vm, heap_ids)?;
            }
        }

        Ok(f.write_char(')')?)
    }

    /// `func`, `args` and `keywords`, all read-only.
    ///
    /// `args` and `keywords` are rebuilt on each access, so mutating the
    /// returned dict does not change what the partial passes on.
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let attr = match attr.static_string() {
            Some(StaticStrings::Func) => "func",
            Some(StaticStrings::Args) => "args",
            Some(StaticStrings::Keywords) => "keywords",
            Some(_) => return Ok(None),
            None => attr.as_str(vm.interns),
        };
        match attr {
            "func" => {
                let func = self.get(vm.heap).func.clone_with_heap(vm);
                Ok(Some(CallResult::Value(func)))
            }
            "args" => {
                check_clone_slots(self.get(vm.heap).args.len(), vm.heap)?;
                let args: SmallVec<_> = (0..self.get(vm.heap).args.len())
                    .map(|index| self.get(vm.heap).args[index].clone_with_heap(vm))
                    .collect();
                Ok(Some(CallResult::Value(allocate_tuple(args, vm.heap))))
            }
            "keywords" => {
                check_clone_slots(self.get(vm.heap).keywords.len().saturating_mul(2), vm.heap)?;
                let pairs: Vec<(Value, Value)> = (0..self.get(vm.heap).keywords.len())
                    .map(|index| {
                        let (name, value) = &self.get(vm.heap).keywords[index];
                        (name.clone_with_heap(vm), value.clone_with_heap(vm))
                    })
                    .collect();
                let dict = Dict::from_pairs(pairs, vm)?;
                Ok(Some(CallResult::Value(Value::Ref(
                    vm.heap.allocate(HeapData::Dict(dict)),
                ))))
            }
            _ => Ok(None),
        }
    }
}
