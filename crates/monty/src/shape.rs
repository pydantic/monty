//! Shared instance attribute layouts for inline attribute lookup.

use std::sync::Arc;

use ahash::AHashMap;

use crate::{
    heap::{Heap, HeapData},
    intern::Interns,
    types::Dict,
    value::Value,
};

/// An immutable layout. Its index in `ShapeRegistry::shapes` is its version.
#[cfg_attr(feature = "baseline-attr-lookup", allow(dead_code))]
#[derive(Debug, Clone, Default)]
struct Shape {
    slots: AHashMap<Arc<str>, usize>,
    transitions: AHashMap<Arc<str>, usize>,
}

/// Session-wide layouts, shared by all instances and code bodies.
///
/// This is derived state: instances forget their cached shape on dump/load, so
/// the registry can be rebuilt on demand without changing the dump format.
#[cfg_attr(feature = "baseline-attr-lookup", allow(dead_code))]
#[derive(Debug, Clone, Default)]
pub(crate) struct ShapeRegistry {
    shapes: Vec<Shape>,
}

#[cfg_attr(feature = "baseline-attr-lookup", allow(dead_code))]
impl ShapeRegistry {
    /// Number of distinct layouts retained by this proof of concept.
    const MAX_SHAPES: usize = 4096;
    const MAX_SLOTS: usize = 32;

    pub(crate) fn slots(&self, shape: usize, name: &str) -> Option<usize> {
        self.shapes.get(shape)?.slots.get(name).copied()
    }

    /// Resolves an instance dict through the transition tree. Returns `None`
    /// when its keys are not strings or the bounded table is full.
    pub(crate) fn shape_for_dict(&mut self, attrs: &Dict, heap: &Heap, interns: &Interns) -> Option<usize> {
        if attrs.len() > Self::MAX_SLOTS {
            return None;
        }
        if self.shapes.is_empty() {
            self.shapes.push(Shape::default());
        }
        let mut shape = 0;
        for (key, _) in attrs {
            let name = match key {
                Value::InternString(id) => interns.get_str(*id),
                Value::Ref(id) => match heap.get(*id) {
                    HeapData::Str(s) => s.as_str(),
                    _ => return None,
                },
                _ => return None,
            };
            if let Some(next) = self.shapes[shape].transitions.get(name).copied() {
                shape = next;
            } else {
                if self.shapes.len() >= Self::MAX_SHAPES {
                    return None;
                }
                let mut slots = self.shapes[shape].slots.clone();
                let name: Arc<str> = Arc::from(name);
                slots.insert(Arc::clone(&name), slots.len());
                let next = self.shapes.len();
                self.shapes.push(Shape {
                    slots,
                    transitions: AHashMap::new(),
                });
                self.shapes[shape].transitions.insert(name, next);
                shape = next;
            }
        }
        Some(shape)
    }
}
