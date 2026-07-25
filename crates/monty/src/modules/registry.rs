//! Open, name-keyed registry of native stdlib modules.
//!
//! This is the extension seam that lets a new native module be added by
//! appending a [`ModuleDescriptor`] literal (in the module's own file) plus a
//! dispatch arm in [`call`], without editing the closed [`StandardLib`] /
//! [`ModuleFunctions`] enums or adding [`crate::intern::StaticStrings`]
//! variants. Module and function names are interned through the dynamic pool —
//! [`crate::prepare`] seeds only the names of the modules a program actually
//! imports (via [`descriptor_names`]), so neither the per-compile seeding cost
//! nor the per-token `StaticStrings::from_str` probe grows as modules land.
//!
//! The registry is split in two:
//! - [`ModuleDescriptor`] / [`REGISTRY`] hold only names and [`ModuleFuncId`]s
//!   (no fn pointers) — plain data in a `static`.
//! - [`call`] resolves an id to its function through a `match`, the simplest
//!   dispatch shape; a fn-pointer table or trait-object registry could replace
//!   the `match` later without touching any caller.
//!
//! [`StandardLib`]: super::StandardLib
//! [`ModuleFunctions`]: super::ModuleFunctions

use std::iter;

use monty_types::ResourceError;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{HeapData, HeapId},
    intern::{Interns, StringId},
    modules::struct_,
    types::Module,
    value::Value,
};

/// Stable identifier for a function in the open module registry.
///
/// The numeric value is a permanent contract: it is stored in
/// [`Value::RegistryFunction`] and folded into object identity
/// (`crate::identity`), so both snapshots and `id()` depend on it. Entries may
/// only be APPENDED to the dispatch `match` in [`call`], never reordered or
/// removed — it is not a live `Vec` position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct ModuleFuncId(pub(crate) u16);

/// Static description of one registered module: its importable name and the
/// `(attribute-name, id)` pairs installed into its namespace at import.
///
/// Holds only interned-name strings and [`ModuleFuncId`]s — plain data with no
/// fn pointers, so it lives in a `static`.
pub(crate) struct ModuleDescriptor {
    /// The importable module name, e.g. `"struct"`.
    pub name: &'static str,
    /// Functions exposed as module attributes, in id order.
    pub functions: &'static [(&'static str, ModuleFuncId)],
}

/// The registered modules. Appending an entry here (and a dispatch arm in
/// [`call`]) is the entire surface for adding a native module.
///
/// Invariant: a descriptor's `name` must not collide with a [`StandardLib`]
/// module name — `load_module` and the compiler gate both try the registry
/// first, so a clash would silently shadow the built-in with no diagnostic.
///
/// [`StandardLib`]: super::StandardLib
pub(crate) static REGISTRY: &[ModuleDescriptor] = &[struct_::DESCRIPTOR];

/// One module's importable name plus every function name it exposes.
///
/// [`crate::prepare`] interns these into the dynamic string pool for each
/// registered module a program imports, so [`create_module`] can resolve the
/// module and all its function names at runtime — including functions the
/// user's source never references by name.
pub(crate) fn descriptor_names(desc: &ModuleDescriptor) -> impl Iterator<Item = &'static str> {
    iter::once(desc.name).chain(desc.functions.iter().map(|(name, _)| *name))
}

/// Resolves a module name string to its descriptor, or `None` if it is not a
/// registered module. The string-keyed core of [`lookup`] and [`is_registered`].
pub(crate) fn lookup_by_name(name: &str) -> Option<&'static ModuleDescriptor> {
    REGISTRY.iter().find(|m| m.name == name)
}

/// Resolves an (already-interned) module name id to its descriptor, or `None`
/// if the name is not a registered module (caller falls back to [`StandardLib`]).
///
/// [`StandardLib`]: super::StandardLib
pub(crate) fn lookup(name_id: StringId, interns: &Interns) -> Option<&'static ModuleDescriptor> {
    lookup_by_name(interns.get_str(name_id))
}

/// Whether `name` is a registered module. Used by the compiler's import gate to
/// decide between emitting `LoadModule` and `RaiseImportError`.
pub(crate) fn is_registered(name: &str) -> bool {
    lookup_by_name(name).is_some()
}

/// The attribute name a function id was registered under, for `repr`.
pub(crate) fn function_name(id: ModuleFuncId) -> &'static str {
    REGISTRY
        .iter()
        .flat_map(|m| m.functions)
        .find(|(_, fid)| *fid == id)
        .map_or("?", |(name, _)| *name)
}

/// Builds a registered module on the heap: a [`Module`] whose namespace maps
/// each function name to a [`Value::RegistryFunction`].
///
/// # Panics
///
/// Panics if a registered name was not pre-interned by [`crate::prepare`]
/// (see [`descriptor_names`]). Since prepare seeds exactly the modules a
/// program imports, this only fires for a module reached without an import.
pub(crate) fn create_module(desc: &ModuleDescriptor, vm: &mut VM<'_>) -> Result<HeapId, ResourceError> {
    let module_name = name_id(desc.name, vm);
    let mut module = Module::new(module_name);
    for (attr, id) in desc.functions {
        let attr_id = name_id(attr, vm);
        module.set_attr(attr_id, Value::RegistryFunction(*id), vm);
    }
    vm.heap.allocate(HeapData::Module(module))
}

/// Dispatches a registry function call by id.
///
/// The dispatch `match` is the sole place a module's functions are referenced;
/// adding a module means appending arms here in id order. Pure functions
/// returning `Value` are wrapped in [`CallResult::Value`].
pub(crate) fn call(id: ModuleFuncId, vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    match id.0 {
        0 => struct_::calcsize(vm, args).map(CallResult::Value),
        1 => struct_::pack(vm, args).map(CallResult::Value),
        2 => struct_::unpack(vm, args).map(CallResult::Value),
        // Only 0..=2 are ever produced by `create_module`; a higher id can only
        // arrive from a corrupt/attacker-supplied deserialized snapshot, so
        // return a catchable error rather than panicking the interpreter.
        other => Err(ExcType::runtime_error(format!("invalid registry function id {other}"))),
    }
}

/// Resolves a registered name to its interned [`StringId`].
///
/// Infallible in practice because [`crate::prepare`] seeds every registered
/// name; the `expect` guards against a missed seeding step rather than user input.
fn name_id(name: &str, vm: &VM<'_>) -> StringId {
    vm.interns
        .get_string_id_by_name(name)
        .expect("registry module/function name must be pre-interned during prepare")
}
