//! Shared compilation and dispatch for internal Python implementations.
//!
//! Internal functions use ordinary VM frames and suspension but are hidden from
//! tracebacks and namespace introspection. Private native helpers arrive as
//! explicit arguments, never through user globals or importable modules.

use monty_types::CompileOptions;
use serde::{Deserialize, Serialize};

use crate::{
    annotations,
    args::ArgValues,
    builtins::next,
    bytecode::{CallResult, Compiler, VM, compile_iteration_catcher},
    exception_private::RunResult,
    intern::{CompileInterns, FunctionId, Interns},
    modules::{functools, typing},
    name_map::NameMap,
    parse::parse_with_interner,
    prepare::prepare_with_existing_names,
    types::{forward_ref, generic_alias, iter, lru_cache, map_filter},
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
    /// Acquire a map's sources without consuming them.
    #[serde(rename = "builtins.map")]
    MapInit,
    /// Advance a map's sources and call its callback once.
    #[serde(rename = "builtins.map_next")]
    MapNext,
    /// Acquire a filter's source without testing its predicate.
    #[serde(rename = "builtins.filter")]
    FilterInit,
    /// Search a filter's source for its next accepted item.
    #[serde(rename = "builtins.filter_next")]
    FilterNext,
    /// Return a default when a suspendable iterator is exhausted.
    #[serde(rename = "builtins.next_default")]
    NextDefault,
    /// Forward a deferred loop item or exit the waiting loop on exhaustion.
    #[serde(rename = "builtins.for_iter")]
    ForIter,
    /// Evaluate annotations for `annotationlib.get_annotations` and `typing.get_type_hints`.
    #[serde(rename = "annotationlib.get_annotations")]
    GetAnnotations,
    /// `ForwardRef.evaluate()`, applying the format's rules to one evaluation.
    #[serde(rename = "annotationlib.ForwardRef.evaluate")]
    ForwardRefEvaluate,
    /// Evaluate and cache a class's `__annotations__` from its `__annotate__`.
    #[serde(rename = "type.__annotations__")]
    ClassAnnotations,
}

impl FrozenFunction {
    /// Publishes the implementation on first use; callers can install its frame without further binding.
    pub(crate) fn resolve(self, interns: &Interns) -> RunResult<FunctionId> {
        if let Some(id) = interns.frozen_function(self) {
            Ok(id)
        } else {
            let (id, overlay) = self.compile(interns)?;
            overlay.commit();
            interns.set_frozen_function(self, id);
            Ok(id)
        }
    }

    /// Prepares an unpublished implementation using this session's intern IDs.
    /// The caller publishes the overlay before installing its frame.
    fn compile(self, interns: &Interns) -> RunResult<(FunctionId, CompileInterns<'_>)> {
        let mut overlay = CompileInterns::new(interns);
        let index = overlay.functions_len();
        let source = match self {
            Self::ForIter => {
                let id = compile_iteration_catcher(&mut overlay)?;
                return Ok((id, overlay));
            }
            Self::FunctoolsReduce => functools::REDUCE_SOURCE,
            Self::Iter => iter::ITER_SOURCE,
            Self::LruCacheMiss => lru_cache::CACHE_MISS_SOURCE,
            Self::NextDefault => next::NEXT_DEFAULT_SOURCE,
            Self::GetAnnotations => typing::GET_ANNOTATIONS_SOURCE,
            Self::ForwardRefEvaluate => forward_ref::EVALUATE_SOURCE,
            Self::ClassAnnotations => annotations::CLASS_ANNOTATIONS_SOURCE,
            Self::MapInit => map_filter::MAP_INIT_SOURCE,
            Self::MapNext => map_filter::MAP_NEXT_SOURCE,
            Self::FilterInit => map_filter::FILTER_INIT_SOURCE,
            Self::FilterNext => map_filter::FILTER_NEXT_SOURCE,
        };
        let mut globals = NameMap::new();
        let parsed =
            parse_with_interner(source, "<monty-frozen>", &mut overlay, false).map_err(|e| e.into_run_error(source))?;
        let nodes =
            prepare_with_existing_names(parsed.nodes, &overlay, &mut globals).map_err(|e| e.into_run_error(source))?;
        Compiler::compile_module(&nodes, &mut overlay, &globals, CompileOptions::default(), false)
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
}

/// Native primitives passed directly to internal Python functions.
/// These identities and their signatures are part of the captured bytecode's ABI.
#[monty_macros::explicit_discriminants(checksum = 0x47d0_77e5_f6a8_58b8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, strum::Display)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum FrozenFunctions {
    /// Rebuild a generic alias around resolved arguments (see `generic_alias::call_alias_with`).
    AliasWith = 0,
    /// Cache a class's evaluated `__annotations__` under `__annotations_cache__`.
    CacheClassAnnotations = 1,
    /// Call an `__annotate__` function in an `annotationlib.Format` (see `annotations::call_annotate_helper`).
    CallAnnotate = 2,
    /// Evaluate one annotation string in explicit namespaces and name-resolution mode.
    EvalHint = 3,
    /// Call a user `__iter__` without validating its result yet.
    GetIter = 4,
    /// Insert a successful result into a cache wrapper.
    Store = 5,
    /// `annotationlib.type_repr`: a type by name, anything else by repr.
    TypeRepr = 6,
    /// Check the result returned by a user `__iter__` method.
    ValidateIterator = 7,
    /// Build a map from already-acquired source iterators.
    BuildMap = 8,
    /// Build a filter from an already-acquired source iterator.
    BuildFilter = 9,
    /// Raise a strict map's length-mismatch error.
    MapLengthError = 10,
    /// Exit the loop beneath a deferred advancement's catcher, without pushing an item.
    EndForIter = 11,
}

/// Dispatches private helpers using the same argument ownership rules as public natives.
pub(crate) fn call_helper(function: FrozenFunctions, args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    match function {
        FrozenFunctions::AliasWith => generic_alias::call_alias_with(args, vm).map(CallResult::Value),
        FrozenFunctions::CacheClassAnnotations => {
            annotations::cache_class_annotations_helper(args, vm).map(CallResult::Value)
        }
        FrozenFunctions::CallAnnotate => annotations::call_annotate_helper(args, vm),
        FrozenFunctions::EvalHint => typing::call_eval_hint(args, vm),
        FrozenFunctions::GetIter => iter::call_get_iter(args, vm),
        FrozenFunctions::Store => lru_cache::call_store(args, vm).map(CallResult::Value),
        FrozenFunctions::TypeRepr => typing::call_type_repr(args, vm).map(CallResult::Value),
        FrozenFunctions::ValidateIterator => iter::call_validate_iterator(args, vm).map(CallResult::Value),
        FrozenFunctions::BuildMap => map_filter::build_map(args, vm).map(CallResult::Value),
        FrozenFunctions::BuildFilter => map_filter::build_filter(args, vm).map(CallResult::Value),
        FrozenFunctions::EndForIter => vm.end_for_iter(args),
        FrozenFunctions::MapLengthError => map_filter::map_length_error(args, vm).map(CallResult::Value),
    }
}
