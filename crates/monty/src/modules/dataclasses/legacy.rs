//! Loading dataclasses from dumps written before `@dataclass` generated methods.
//!
//! Dump version 13 builds (1.0 and 1.1) decorated a class by writing only
//! `__dataclass_fields__` and `__dataclass_params__`; the VM read both at every
//! construction, comparison, hash, repr and assignment. Today the decorator also
//! writes `DataclassMethod`s, and a class without them behaves as a plain one.
//! A [`crate::dump_upgrade`] generates the missing methods from those two
//! entries, which hold everything decoration saw.
//!
//! Everything here exists only for that compatibility, so it can be deleted
//! together once `MIN_SUPPORTED_DUMP_VERSION` passes [`LEGACY_DUMP_VERSION`].

use super::{DataclassOptions, Installer, install_generated_methods};
use crate::{
    heap::{DropWithContext, Heap, HeapData, HeapId, HeapReadOutput, HeapReader},
    intern::Interns,
    types::instance::namespace_member,
    value::Value,
};

/// The last dump version written before `@dataclass` generated methods.
pub(crate) const LEGACY_DUMP_VERSION: u16 = 13;

/// Generates the methods of every dataclass on a heap loaded from a version 13
/// dump, as [`crate::dump_upgrade`] calls before the session is handed out.
pub(crate) fn upgrade_legacy_classes(heap: &mut Heap, interns: &Interns) {
    let class_ids = heap.ids_where(|data| matches!(data, HeapData::Class(_)));
    HeapReader::with(heap, &mut (), |heap, ()| {
        // Owned references keep every class alive until all are upgraded:
        // replacing one class's hook can drop the last reference to another.
        let classes: Vec<Value> = class_ids
            .into_iter()
            .map(|id| {
                heap.inc_ref(id);
                Value::Ref(id)
            })
            .collect();
        for class in &classes {
            let Value::Ref(class_id) = *class else {
                unreachable!("collected as references")
            };
            upgrade_class(class_id, heap, interns);
        }
        classes.drop_with(heap);
    });
}

/// Generates the methods of `class_id` if it is a dataclass.
fn upgrade_class(class_id: HeapId, heap: &mut HeapReader<'_>, interns: &Interns) {
    let Some((fields_id, options)) = legacy_dataclass(class_id, heap, interns) else {
        return;
    };
    let HeapReadOutput::Class(class) = heap.read(class_id) else {
        unreachable!("filtered to classes")
    };
    let mut installer = Installer::Upgrade { heap, interns };
    install_generated_methods(class_id, &class, fields_id, options, &mut installer)
        .expect("an upgrade's writes skip the memory preflight, so cannot fail");
}

/// The fields mapping and options of a dataclass, or `None` for any other class.
fn legacy_dataclass(class_id: HeapId, heap: &Heap, interns: &Interns) -> Option<(HeapId, DataclassOptions)> {
    let options = match namespace_member(class_id, "__dataclass_params__", heap, interns) {
        Some(Value::Ref(id)) => match heap.get(*id) {
            HeapData::DataclassParams(params) => params.options(),
            _ => return None,
        },
        _ => return None,
    };
    match namespace_member(class_id, "__dataclass_fields__", heap, interns) {
        Some(Value::Ref(id)) if matches!(heap.get(*id), HeapData::Dict(_)) => Some((*id, options)),
        _ => None,
    }
}
