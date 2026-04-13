//! Implementation of the `sys` module.
//!
//! Provides a minimal implementation of Python's `sys` module with:
//! - `version`: Python version string (e.g., "3.14.0 (Monty)")
//! - `version_info`: Named tuple (3, 14, 0, 'final', 0)
//! - `platform`: Platform identifier ("monty")
//! - `stdout`: Marker for standard output (no real functionality)
//! - `stderr`: Marker for standard error (no real functionality)

use crate::{
    bytecode::VM,
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    resource::{ResourceError, ResourceTracker},
    types::{Module, NamedTuple},
    value::{Marker, Value},
};

/// Creates the `sys` module and allocates it on the heap.
///
/// Returns a HeapId pointing to the newly allocated module.
///
/// # Panics
///
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Sys);

    // sys.platform
    module.set_attr(StaticStrings::Platform, StaticStrings::Monty.into(), vm);

    // sys.stdout / sys.stderr - markers for standard output/error
    module.set_attr(StaticStrings::Stdout, Value::Marker(Marker(StaticStrings::Stdout)), vm);
    module.set_attr(StaticStrings::Stderr, Value::Marker(Marker(StaticStrings::Stderr)), vm);

    // sys.version
    module.set_attr(StaticStrings::Version, StaticStrings::MontyVersionString.into(), vm);
    // sys.version_info - named tuple (major=3, minor=14, micro=0, releaselevel='final', serial=0)
    let version_info = NamedTuple::new(
        StaticStrings::SysVersionInfo,
        vec![
            StaticStrings::Major.into(),
            StaticStrings::Minor.into(),
            StaticStrings::Micro.into(),
            StaticStrings::Releaselevel.into(),
            StaticStrings::Serial.into(),
        ],
        vec![
            Value::Int(3),
            Value::Int(14),
            Value::Int(0),
            Value::InternString(StaticStrings::Final.into()),
            Value::Int(0),
        ],
    );
    let version_info_id = vm.heap.allocate(HeapData::NamedTuple(version_info))?;
    module.set_attr(StaticStrings::VersionInfo, Value::Ref(version_info_id), vm);

    vm.heap.allocate(HeapData::Module(module))
}
