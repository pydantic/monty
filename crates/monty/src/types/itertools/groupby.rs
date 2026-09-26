//! `itertools.groupby(iterable, key=None)` — runs of equal keys, each as a
//! `(key, group)` pair whose group is a sub-iterator over that run.
//!
//! The two types here are one state machine: [`GroupBy`] owns the source and
//! the item read ahead from it, and each [`Grouper`] it yields draws from that
//! read-ahead through its parent. Only the most recently yielded grouper may
//! do so — advancing the `groupby` spends it, as CPython's `currgrouper` check
//! does, so a group kept from earlier is empty rather than out of order.

use serde::{Deserialize, Serialize};

use crate::{
    args::ArgValues,
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    heap::{DropGuard, DropWithContext, HeapData, HeapId, HeapObjectRead, HeapRead, HeapReadOutput},
    types::{
        allocate_tuple,
        itertools::{ItertoolsIter, step::next_item},
    },
    value::Value,
};

/// The outer iterator: yields `(key, grouper)` once per run of equal keys.
///
/// Boxed inside [`ItertoolsIter`]: six fields put it well past the family's
/// size budget.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct GroupBy {
    source: Value,
    /// `Value::None` groups by the items themselves, as CPython's absent
    /// `keyfunc` does.
    keyfunc: Value,
    /// The key of the group most recently yielded — CPython's `tgtkey`.
    target_key: Option<Value>,
    /// The key and item read ahead from the source and not yet handed to a
    /// group. Always set and cleared together — [`step`] reads a pair in, and
    /// a grouper takes both as it hands the item out.
    current_key: Option<Value>,
    current_value: Option<Value>,
    /// The grouper most recently yielded, the only one allowed to draw from
    /// the source. Identity only — a borrowed id, never released here. It can
    /// only ever match a live grouper of this `groupby`: a later one replaces
    /// it, so a stale id names no live grouper that would check it.
    current_grouper: Option<HeapId>,
}

impl GroupBy {
    /// Takes ownership of both, with `source` already resolved by `py_iter`.
    pub(crate) fn new(source: Value, keyfunc: Value) -> Self {
        Self {
            source,
            keyfunc,
            target_key: None,
            current_key: None,
            current_value: None,
            current_grouper: None,
        }
    }

    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        for value in [&self.source, &self.keyfunc] {
            if let Value::Ref(id) = value {
                on_child(*id);
            }
        }
        for value in [&self.target_key, &self.current_key, &self.current_value] {
            if let Some(Value::Ref(id)) = value {
                on_child(*id);
            }
        }
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.source.py_dec_ref_ids(stack);
        self.keyfunc.py_dec_ref_ids(stack);
        for value in [&mut self.target_key, &mut self.current_key, &mut self.current_value]
            .into_iter()
            .flatten()
        {
            value.py_dec_ref_ids(stack);
        }
    }
}

/// The inner iterator: the items of one group, drawn through the parent.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Grouper {
    /// The `groupby` this group belongs to — owned, as CPython's `parent` is,
    /// so the group outlives a caller that drops the outer iterator.
    parent: Value,
    /// The key every item of this group must equal.
    target_key: Value,
}

impl Grouper {
    /// Invokes `on_child` for each heap id this iterator owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        for value in [&self.parent, &self.target_key] {
            if let Value::Ref(id) = value {
                on_child(*id);
            }
        }
    }

    /// Releases the refs this iterator owns (mirrors `for_each_child_id`).
    pub(crate) fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.parent.py_dec_ref_ids(stack);
        self.target_key.py_dec_ref_ids(stack);
    }
}

/// Skips whatever is left of the current group, then opens the next one.
///
/// Takes the object read rather than the bare handle because the new grouper
/// needs an owned reference to this `groupby`.
pub(super) fn next<'h>(iter: &mut HeapObjectRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
    // Spent from here on, whether or not a new group opens: CPython clears
    // `currgrouper` before the skip.
    groupby_mut(iter, vm).current_grouper = None;

    let mut steps = 0usize;
    loop {
        // A native loop discarding items: poll so the time limits still bite
        // on a source whose key never changes (see `chain::next`).
        vm.heap.tracker.check_time_every(steps)?;
        steps += 1;
        let groupby = groupby_ref(iter, vm);
        let current = groupby.current_key.as_ref().map(|key| key.clone_with_heap(vm.heap));
        let target = groupby.target_key.as_ref().map(|key| key.clone_with_heap(vm.heap));
        defer_drop!(current, vm);
        defer_drop!(target, vm);
        let same_group = match (current, target) {
            // Nothing read ahead: take the first item of whatever comes next.
            (None, _) => true,
            // Read ahead, and no group opened yet: it starts one.
            (Some(_), None) => false,
            // Read ahead: still the open group's, or the start of the next. The
            // target is the LEFT operand, as CPython's comparison makes it.
            (Some(current), Some(target)) => target.py_eq(current, vm)?,
        };
        // A user `__eq__` can step this same `groupby` and consume the pair
        // that was just compared, so the answer is only usable while a key is
        // still there. CPython re-tests `currkey` at the top of its loop for
        // the same reason; without this the read below has nothing to take.
        if !same_group && groupby_ref(iter, vm).current_key.is_some() {
            break;
        }
        if !step(iter, vm)? {
            return Ok(None);
        }
    }

    // The read-ahead key is the new group's: yielded, kept as the target, and
    // given to the grouper — three references, all taken before the `&mut`.
    let groupby = groupby_ref(iter, vm);
    let key = groupby
        .current_key
        .as_ref()
        .expect("the skip loop only breaks with a key read ahead");
    let (yielded, target, group_target) = (
        key.clone_with_heap(vm.heap),
        key.clone_with_heap(vm.heap),
        key.clone_with_heap(vm.heap),
    );
    let grouper = Grouper {
        parent: iter.clone_value(vm.heap),
        target_key: group_target,
    };
    let grouper_id = vm.heap.allocate(HeapData::Itertools(ItertoolsIter::Grouper(grouper)));
    let groupby = groupby_mut(iter, vm);
    groupby.current_grouper = Some(grouper_id);
    let previous_target = groupby.target_key.replace(target);
    previous_target.drop_with(vm);
    Ok(Some(allocate_tuple(
        [yielded, Value::Ref(grouper_id)].into_iter().collect(),
        vm.heap,
    )))
}

/// Hands out the read-ahead item if it belongs to this group, reading one in
/// first when the parent holds none.
pub(super) fn grouper_next<'h>(
    iter: &mut HeapObjectRead<'h, ItertoolsIter>,
    vm: &mut VM<'h>,
) -> RunResult<Option<Value>> {
    let ItertoolsIter::Grouper(grouper) = iter.get(vm.heap) else {
        unreachable!("dispatched on Kind::Grouper")
    };
    let own_id = iter.id();
    let parent = grouper.parent.clone_with_heap(vm.heap);
    let target = grouper.target_key.clone_with_heap(vm.heap);
    defer_drop!(parent, vm);
    defer_drop!(target, vm);
    let Value::Ref(parent_id) = parent else {
        unreachable!("a grouper's parent is always a heap groupby")
    };
    let HeapReadOutput::Itertools(mut parent) = vm.heap.read(*parent_id) else {
        unreachable!("a grouper's parent is always a groupby")
    };

    let groupby = groupby_ref(&parent, vm);
    // A group left behind by advancing the parent is spent, not resumed. Only
    // on entry, as CPython's `_grouper_next` tests it: a key function that
    // advances the parent mid-step leaves this group still yielding there, and
    // re-testing afterwards would stop it a round early.
    if groupby.current_grouper != Some(own_id) {
        return Ok(None);
    }
    if groupby.current_value.is_none() && !step(&mut parent, vm)? {
        return Ok(None);
    }
    let current = groupby_ref(&parent, vm)
        .current_key
        .as_ref()
        .expect("step stores a key with every value")
        .clone_with_heap(vm.heap);
    defer_drop!(current, vm);
    if !target.py_eq(current, vm)? {
        return Ok(None);
    }
    // The item leaves with the grouper; its key is cleared so the parent reads
    // the next pair in rather than comparing a key whose item is gone.
    let groupby = groupby_mut(&mut parent, vm);
    let value = groupby.current_value.take();
    let key = groupby.current_key.take();
    key.drop_with(vm);
    Ok(value)
}

/// Reads the next item in and keys it, replacing the read-ahead pair.
///
/// `false` means the source is spent; the pair already held is left as it was,
/// as CPython's `groupby_step` leaves `currkey` and `currvalue`.
fn step<'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &mut VM<'h>) -> RunResult<bool> {
    let groupby = groupby_ref(iter, vm);
    let source = groupby.source.clone_with_heap(vm.heap);
    let keyfunc = groupby.keyfunc.clone_with_heap(vm.heap);
    defer_drop!(keyfunc, vm);
    let Some(item) = next_item(source, vm)? else {
        return Ok(false);
    };
    // Guarded: the key function re-enters the VM and may raise with the item held.
    let mut item_guard = DropGuard::new(item, vm);
    let (item, vm) = item_guard.as_parts_mut();
    let key = if matches!(keyfunc, Value::None) {
        item.clone_with_heap(vm.heap)
    } else {
        let arg = item.clone_with_heap(vm.heap);
        vm.evaluate_function("groupby()", keyfunc, ArgValues::One(arg))?
    };
    let (item, vm) = item_guard.into_parts();
    let groupby = groupby_mut(iter, vm);
    let previous_key = groupby.current_key.replace(key);
    let previous_value = groupby.current_value.replace(item);
    previous_key.drop_with(vm);
    previous_value.drop_with(vm);
    Ok(true)
}

/// The `GroupBy` behind an iterator already dispatched as one.
fn groupby_ref<'r, 'h>(iter: &HeapRead<'h, ItertoolsIter>, vm: &'r VM<'h>) -> &'r GroupBy {
    let ItertoolsIter::GroupBy(groupby) = iter.get(vm.heap) else {
        unreachable!("dispatched on Kind::GroupBy")
    };
    groupby
}

/// Mutable counterpart of [`groupby_ref`].
fn groupby_mut<'r, 'h>(iter: &mut HeapRead<'h, ItertoolsIter>, vm: &'r mut VM<'h>) -> &'r mut GroupBy {
    let ItertoolsIter::GroupBy(groupby) = iter.get_mut(vm.heap) else {
        unreachable!("dispatched on Kind::GroupBy")
    };
    groupby
}
