//! Attribute access helpers for the VM.

use super::VM;
use crate::{
    bytecode::vm::CallResult,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError},
    heap::{ContainsHeap, DropWithContext},
    intern::StringId,
    value::{EitherStr, Value},
};

#[cfg(not(feature = "baseline-attr-lookup"))]
use crate::heap::HeapReadOutput;

#[cfg(all(not(feature = "baseline-attr-lookup"), feature = "simple-attr-cache"))]
#[derive(Clone, Copy)]
pub(super) struct AttrCache {
    shape: usize,
    slot: usize,
}

#[cfg(all(not(feature = "baseline-attr-lookup"), not(feature = "simple-attr-cache")))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AttrCache {
    Active {
        seen: [u16; 4],
        count: u8,
        slot: Option<(u16, u8)>,
        miss: Option<u16>,
    },
    Megamorphic,
}

#[cfg(all(not(feature = "baseline-attr-lookup"), not(feature = "simple-attr-cache")))]
impl AttrCache {
    fn observe(self, shape: u16) -> Self {
        match self {
            Self::Active {
                mut seen,
                mut count,
                slot,
                miss,
            } => {
                if !seen[..usize::from(count)].contains(&shape) {
                    if usize::from(count) == seen.len() {
                        return Self::Megamorphic;
                    }
                    seen[usize::from(count)] = shape;
                    count += 1;
                }
                Self::Active {
                    seen,
                    count,
                    slot,
                    miss,
                }
            }
            Self::Megamorphic => Self::Megamorphic,
        }
    }
}

/// What a suspended lazy attribute lookup does with the host's answer when it
/// resumes, instead of pushing the value (or raising `AttributeError` on
/// `Undefined`) the way `obj.attr` does.
///
/// Produced by the `getattr()` / `hasattr()` builtins. It rides on
/// [`CallResult::AttrLookup`] and `FrameExit::AttrLookup`, and is armed on
/// [`VM::pending_lookup_effect`] once the lookup reaches the host, so it
/// survives a dump/restore of the suspended session. A lookup that never
/// reaches a host (no host, or a synchronous nested call) is `Undefined`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum PendingLookupEffect {
    /// `hasattr()`: push `True` for a served value (which is dropped),
    /// `False` for `Undefined`.
    HasAttr,
    /// `getattr(obj, name, default)`: push `default` for `Undefined`. Owns
    /// the heap reference until the resume consumes it.
    Default(Value),
}

impl PendingLookupEffect {
    /// The value to push for the host's answer: `Some(value)` for a served
    /// attribute, `None` for `Undefined`.
    pub(crate) fn apply(self, answer: Option<Value>, vm: &mut VM<'_>) -> Value {
        match (self, answer) {
            (Self::HasAttr, Some(value)) => {
                value.drop_with(vm);
                Value::Bool(true)
            }
            (Self::HasAttr, None) => Value::Bool(false),
            (Self::Default(default), Some(value)) => {
                default.drop_with(vm);
                value
            }
            (Self::Default(default), None) => default,
        }
    }
}

impl<C: ContainsHeap> DropWithContext<C> for PendingLookupEffect {
    fn drop_with(self, heap: &mut C) {
        if let Self::Default(value) = self {
            value.drop_with(heap);
        }
    }
}

impl VM<'_> {
    /// Original attribute lookup for the benchmark control build.
    #[cfg(feature = "baseline-attr-lookup")]
    pub(super) fn load_attr(&mut self, name_id: StringId) -> Result<CallResult, RunError> {
        let this = self;
        let obj = this.pop();
        defer_drop!(obj, this);
        obj.py_getattr(&EitherStr::Interned(name_id), this)
    }

    /// The simple monomorphic shape cache, retained for benchmarks.
    #[cfg(all(not(feature = "baseline-attr-lookup"), feature = "simple-attr-cache"))]
    pub(super) fn load_attr_cached(&mut self, name_id: StringId) -> Result<CallResult, RunError> {
        let this = self;
        let obj = this.pop();
        defer_drop!(obj, this);

        if let Value::Ref(id) = obj
            && let HeapReadOutput::Instance(instance) = this.heap.read(*id)
        {
            let shape = instance.get(this.heap).shape().or_else(|| {
                let attrs = instance.get(this.heap).attrs();
                let shape = this.shapes.shape_for_dict(attrs, this.heap, this.interns);
                instance.get(this.heap).set_shape(shape);
                shape
            });
            if let Some(shape) = shape {
                let code = this.current_frame.code as *const _ as usize;
                let ip = this.instruction_ip;
                let cached = this
                    .attr_caches
                    .get(&code)
                    .and_then(|entries| entries.get(ip))
                    .copied()
                    .flatten();
                let slot = match cached {
                    Some(entry) if entry.shape == shape => Some(entry.slot),
                    _ => this.shapes.slots(shape, this.interns.get_str(name_id)),
                };
                if let Some(slot) = slot {
                    if cached.is_none_or(|entry| entry.shape != shape) {
                        let entries = this.attr_caches.entry(code).or_default();
                        if entries.len() <= ip {
                            entries.resize(ip + 1, None);
                        }
                        entries[ip] = Some(AttrCache { shape, slot });
                    }
                    if let Some(value) = instance.get(this.heap).attrs().value_at(slot) {
                        return Ok(CallResult::Value(value.clone_with_heap(this.heap)));
                    }
                }
            }
        }

        obj.py_getattr(&EitherStr::Interned(name_id), this)
    }

    /// Directly reads a dict slot when the receiver has the cached layout.
    #[cfg(all(not(feature = "baseline-attr-lookup"), not(feature = "simple-attr-cache")))]
    pub(super) fn load_attr_cached(&mut self, name_id: StringId) -> Result<CallResult, RunError> {
        let this = self;
        let obj = this.pop();
        defer_drop!(obj, this);

        let code = this.current_frame.code as *const _ as usize;
        let ip = this.instruction_ip;
        let cache_entry = this
            .attr_caches
            .get(&code)
            .and_then(|entries| entries.get(ip))
            .and_then(Option::as_ref);
        if matches!(cache_entry, Some(AttrCache::Megamorphic)) {
            return obj.py_getattr(&EitherStr::Interned(name_id), this);
        }
        let cached = cache_entry.copied();

        if let Value::Ref(id) = obj
            && let HeapReadOutput::Instance(instance) = this.heap.read(*id)
        {
            let shape = instance.get(this.heap).shape().or_else(|| {
                let attrs = instance.get(this.heap).attrs();
                let shape = this.shapes.shape_for_dict(attrs, this.heap, this.interns);
                instance.get(this.heap).set_shape(shape);
                shape
            });
            if let Some(shape) = shape {
                let shape = u16::try_from(shape).expect("shape registry is capped at 4096 entries");
                if let Some(AttrCache::Active { miss: Some(miss), .. }) = cached
                    && miss == shape
                {
                    return obj.py_getattr(&EitherStr::Interned(name_id), this);
                }
                if let Some(AttrCache::Active {
                    slot: Some((cached_shape, slot)),
                    ..
                }) = cached
                    && cached_shape == shape
                    && let Some(value) = instance.get(this.heap).attrs().value_at(usize::from(slot))
                {
                    return Ok(CallResult::Value(value.clone_with_heap(this.heap)));
                }
                let state = cached.unwrap_or(AttrCache::Active {
                    seen: [0; 4],
                    count: 0,
                    slot: None,
                    miss: None,
                });
                let state = state.observe(shape);
                let state = match state {
                    AttrCache::Active {
                        seen,
                        count,
                        slot,
                        miss,
                    } => {
                        let found = slot
                            .filter(|(cached_shape, _)| *cached_shape == shape)
                            .map(|(_, slot)| slot)
                            .or_else(|| {
                                this.shapes
                                    .slots(usize::from(shape), this.interns.get_str(name_id))
                                    .map(|slot| u8::try_from(slot).expect("shapes have at most 32 slots"))
                            });
                        AttrCache::Active {
                            seen,
                            count,
                            slot: found.map(|index| (shape, index)).or(slot),
                            miss: if found.is_none() { Some(shape) } else { miss },
                        }
                    }
                    AttrCache::Megamorphic => AttrCache::Megamorphic,
                };
                if cached != Some(state) {
                    let entries = this.attr_caches.entry(code).or_default();
                    if entries.len() <= ip {
                        entries.resize(ip + 1, None);
                    }
                    entries[ip] = Some(state);
                }
                if let AttrCache::Active {
                    slot: Some((cached_shape, slot)),
                    ..
                } = state
                    && cached_shape == shape
                    && let Some(value) = instance.get(this.heap).attrs().value_at(usize::from(slot))
                {
                    return Ok(CallResult::Value(value.clone_with_heap(this.heap)));
                }
            }
        }

        obj.py_getattr(&EitherStr::Interned(name_id), this)
    }

    /// Loads an attribute from a module for `from ... import` and pushes it onto the stack.
    ///
    /// Returns an ImportError (not AttributeError) if the attribute doesn't exist,
    /// matching CPython's behavior for `from module import name`.
    pub(super) fn load_attr_import(&mut self, name_id: StringId) -> Result<CallResult, RunError> {
        let this = self;

        let obj = this.pop();
        defer_drop!(obj, this);

        let attr = EitherStr::Interned(name_id);
        match obj.py_getattr(&attr, this) {
            Ok(result) => Ok(result),
            Err(RunError::Exc(exc)) if exc.exc.exc_type() == ExcType::AttributeError => {
                // Only compute module_name when we need it for the error message
                let module_name = obj.module_name(this);
                let name_str = this.interns.get_str(name_id);
                Err(ExcType::cannot_import_name(name_str, &module_name))
            }
            Err(e) => Err(e),
        }
    }

    /// Stores a value as an attribute on an object.
    ///
    /// Returns an AttributeError if the attribute cannot be set.
    pub(super) fn store_attr(&mut self, name_id: StringId) -> Result<(), RunError> {
        let this = self;

        let obj = this.pop();
        defer_drop!(obj, this);

        let value = this.pop();
        // py_set_attr takes ownership of value and drops it on error
        obj.py_set_attr(&EitherStr::Interned(name_id), value, this)
    }
}
