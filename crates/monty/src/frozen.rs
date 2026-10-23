//! Shared compilation and dispatch for internal Python implementations.
//!
//! Internal functions use ordinary VM frames and suspension but are hidden from
//! tracebacks and namespace introspection. Private native helpers arrive as
//! explicit arguments, never through user globals or importable modules.

use monty_types::CompileOptions;
use serde::{Deserialize, Serialize};

use crate::{
    args::ArgValues,
    builtins::next,
    bytecode::{CallResult, Compiler, VM},
    exception_private::RunResult,
    intern::{CompileInterns, FunctionId, Interns},
    modules::functools,
    name_map::NameMap,
    parse::parse_with_interner,
    prepare::prepare_with_existing_names,
    types::{builtin_descriptor::DescriptorMethod, descriptor, iter, lru_cache},
};

/// Identifies an internal implementation with one authoritative source definition.
/// Qualified names persist in dumps; the registry uses compact enum keys at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum FrozenFunction {
    /// Suspendable fold loop behind `functools.reduce`.
    #[serde(rename = "functools.reduce")]
    FunctoolsReduce,
    /// Validate the result of a suspendable user `__iter__` method.
    #[serde(rename = "builtins.iter")]
    Iter,
    /// Store a cached call's result only after it returns successfully.
    #[serde(rename = "functools._lru_cache_miss")]
    LruCacheMiss,
    /// Return a default when a suspendable user `__next__` method is exhausted.
    #[serde(rename = "builtins.next_default")]
    NextDefault,
    #[serde(rename = "descriptors.call")]
    DescriptorCall,
    #[serde(rename = "descriptors.bind_call")]
    DescriptorBindCall,
    #[serde(rename = "descriptors.discard")]
    DescriptorDiscard,
    #[serde(rename = "descriptors.mutation")]
    DescriptorMutation,
    #[serde(rename = "descriptors.set_names")]
    DescriptorNames,
    #[serde(rename = "descriptors.getattr")]
    DescriptorGetattr,
    #[serde(rename = "descriptors.hasattr")]
    DescriptorHasattr,
    #[serde(rename = "descriptors.match")]
    DescriptorMatch,
    #[serde(rename = "descriptors.match_class")]
    DescriptorMatchClass,
}

impl FrozenFunction {
    /// Prepares an unpublished implementation using this session's intern IDs.
    /// The caller publishes the overlay before installing its frame.
    pub(crate) fn compile(self, interns: &Interns) -> RunResult<(FunctionId, CompileInterns<'_>)> {
        let source = self.source();
        let mut overlay = CompileInterns::new(interns);
        let index = overlay.functions_len();
        let mut globals = NameMap::new();
        let nodes =
            parse_with_interner(source, "<monty-frozen>", &mut overlay).map_err(|e| e.into_run_error(source))?;
        let nodes = prepare_with_existing_names(nodes, &overlay, &mut globals).map_err(|e| e.into_run_error(source))?;
        Compiler::compile_module(&nodes, &mut overlay, &globals, CompileOptions::default())
            .map_err(|e| e.into_run_error(source))?;
        assert_eq!(
            overlay.functions_len(),
            index + 1,
            "frozen source must define one function"
        );
        assert_eq!(
            globals.len(),
            1,
            "frozen source must take its dependencies as arguments"
        );
        // Successful compilation checked the bytecode's function-ID limit.
        let id = FunctionId::from_index(u16::try_from(index).expect("compiled function ID fits u16"));
        Ok((id, overlay))
    }

    /// Selects one self-contained function; dependencies arrive as arguments, not globals.
    fn source(self) -> &'static str {
        match self {
            Self::FunctoolsReduce => functools::REDUCE_SOURCE,
            Self::Iter => iter::ITER_SOURCE,
            Self::LruCacheMiss => lru_cache::CACHE_MISS_SOURCE,
            Self::NextDefault => next::NEXT_DEFAULT_SOURCE,
            Self::DescriptorBindCall => descriptor::BIND_CALL_SOURCE,
            Self::DescriptorCall => descriptor::CALL_SOURCE,
            Self::DescriptorMutation => descriptor::MUTATION_SOURCE,
            Self::DescriptorDiscard => descriptor::DISCARD_SOURCE,
            Self::DescriptorNames => descriptor::NAMES_SOURCE,
            Self::DescriptorGetattr => descriptor::GETATTR_SOURCE,
            Self::DescriptorMatch => descriptor::MATCH_SOURCE,
            Self::DescriptorMatchClass => descriptor::MATCH_CLASS_SOURCE,
            Self::DescriptorHasattr => descriptor::HASATTR_SOURCE,
        }
    }
}

/// Native primitives passed directly to internal Python functions.
/// These identities and their signatures are part of the captured bytecode's ABI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, strum::Display)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum FrozenFunctions {
    /// Call a user `__iter__` without validating its result yet.
    GetIter,
    /// Insert a successful result into a cache wrapper.
    Store,
    /// Check the result returned by a user `__iter__` method.
    ValidateIterator,
    GetAttribute,
    BindDescriptor,
    SetNameLookup,
    MatchClassNames,
    Descriptor(DescriptorMethod),
}

/// Dispatches private helpers using the same argument ownership rules as public natives.
pub(crate) fn call_helper(function: FrozenFunctions, args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    match function {
        FrozenFunctions::MatchClassNames => vm.match_class_names(args),
        FrozenFunctions::SetNameLookup => descriptor::lookup_set_name(args, vm),
        FrozenFunctions::BindDescriptor => descriptor::bind_descriptor(args, vm),
        FrozenFunctions::GetAttribute => descriptor::get_attribute(args, vm),
        FrozenFunctions::Descriptor(method) => method.call(vm, args),
        FrozenFunctions::GetIter => iter::call_get_iter(args, vm),
        FrozenFunctions::Store => lru_cache::call_store(args, vm).map(CallResult::Value),
        FrozenFunctions::ValidateIterator => iter::call_validate_iterator(args, vm).map(CallResult::Value),
    }
}
