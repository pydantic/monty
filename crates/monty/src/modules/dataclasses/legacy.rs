//! Loading dataclasses from dumps written before `@dataclass` generated methods.
//!
//! Dump version 13 builds (1.0 and 1.1) decorated a class by writing only
//! `__dataclass_fields__` and `__dataclass_params__`; the VM read both at every
//! construction, comparison, hash, repr and assignment. Today the decorator also
//! writes `DataclassMethod`s, and a class without them behaves as a plain one.
//! Rather than bump `DUMP_VERSION`, a [`crate::dump_upgrade`] generates the missing
//! methods from those two entries, which hold everything decoration saw.
//!
//! Everything here exists only for that compatibility, so it can be deleted
//! together once version 13 dumps no longer need to load.

use super::{DataclassOptions, DataclassParams, InstallMode, install_generated_methods, set_member};
use crate::{
    bytecode::VM,
    heap::{HeapData, HeapId, HeapReadOutput},
    intern::StaticStrings,
    types::instance::class_dunder,
    value::Value,
};

/// Generates the methods of `class_id` if an older build decorated it, as
/// [`crate::dump_upgrade`] calls for each restored class.
///
/// Such a class's `__dataclass_params__` reads back with no `generated` flag.
/// The upgrade gives the class fresh params that have it, so a re-dump of the
/// upgraded session never upgrades it twice, even if params were shared.
pub(crate) fn upgrade_legacy_class(class_id: HeapId, vm: &mut VM<'_>) {
    let Some((fields_id, options)) = legacy_dataclass(class_id, vm) else {
        return;
    };
    let HeapReadOutput::Class(mut class) = vm.heap.read(class_id) else {
        unreachable!("restore listed a class")
    };
    let params = vm.heap.allocate_as(DataclassParams::new(options)).into_value();
    install_generated_methods(class_id, &mut class, fields_id, options, InstallMode::Upgrade, vm)
        .and_then(|()| {
            set_member(
                &mut class,
                StaticStrings::DataclassParams,
                params,
                InstallMode::Upgrade,
                vm,
            )
        })
        .expect("an upgrade writes interned keys without a memory preflight, which cannot fail");
}

/// The fields mapping and options of a dataclass an older build decorated, or
/// `None` for any other class.
fn legacy_dataclass(class_id: HeapId, vm: &VM<'_>) -> Option<(HeapId, DataclassOptions)> {
    let options = match class_dunder(class_id, "__dataclass_params__", vm) {
        Some(Value::Ref(id)) => match vm.heap.get(*id) {
            HeapData::DataclassParams(params) => params.legacy_options()?,
            _ => return None,
        },
        _ => return None,
    };
    match class_dunder(class_id, "__dataclass_fields__", vm) {
        Some(Value::Ref(id)) if matches!(vm.heap.get(*id), HeapData::Dict(_)) => Some((*id, options)),
        _ => None,
    }
}
