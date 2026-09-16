//! Conversions for resume payloads: the parent's answers to suspension
//! events (`ResumeCall`, `ResumeNameLookup`, `ResumeFutures`). A returned
//! value is an index into the request's arena, so the arena travels alongside.

use monty_types::{ExtFunctionResult, MontyException, MontyGraph, MontyObject, NameLookupResult, NodeId};

use crate::{
    convert::{ProtoConvertError, arena_or_empty, value_from_parts},
    pb,
    wire::{WireArena, graph_error},
};

/// Projects a call result onto the wire, with the arena its value indexes
/// (`None` for the arms that carry no value).
#[must_use]
pub fn ext_result_to_proto(result: ExtFunctionResult) -> (pb::ExtFunctionResult, Option<WireArena>) {
    let (kind, values) = match result {
        ExtFunctionResult::Return(value) => (
            pb::ext_function_result::Kind::ReturnValue(value.root.0),
            Some(WireArena::new(value.graph)),
        ),
        ExtFunctionResult::Error(exc) => (pb::ext_function_result::Kind::Error((&exc).into()), None),
        ExtFunctionResult::Future(call_id) => (pb::ext_function_result::Kind::Future(call_id), None),
        ExtFunctionResult::NotFound(name) => (pb::ext_function_result::Kind::NotFound(name), None),
    };
    (pb::ExtFunctionResult { kind: Some(kind) }, values)
}

/// Validates a decoded call result against the arena its message carried.
pub fn ext_result_from_proto(
    result: pb::ExtFunctionResult,
    values: Option<WireArena>,
) -> Result<ExtFunctionResult, ProtoConvertError> {
    let kind = result
        .kind
        .ok_or(ProtoConvertError::MissingField("ExtFunctionResult.kind"))?;
    match kind {
        pb::ext_function_result::Kind::ReturnValue(root) => {
            Ok(ExtFunctionResult::Return(value_from_parts(values, root, "values")?))
        }
        pb::ext_function_result::Kind::Error(err) => Ok(ExtFunctionResult::Error(MontyException::try_from(err)?)),
        pb::ext_function_result::Kind::Future(call_id) => Ok(ExtFunctionResult::Future(call_id)),
        pb::ext_function_result::Kind::NotFound(name) => Ok(ExtFunctionResult::NotFound(name)),
        // NotHandled has no monty equivalent: it resolves against the
        // suspended OS call, so the child intercepts it before this
        // conversion (see `Child::handle_resume_call`); anywhere else it
        // is out of context
        pb::ext_function_result::Kind::NotHandled(_) => Err(ProtoConvertError::InvalidValue {
            field: "ExtFunctionResult.kind",
            reason: "NotHandled is only valid answering a suspended OS call".to_owned(),
        }),
    }
}

impl From<NameLookupResult> for pb::ResumeNameLookup {
    fn from(result: NameLookupResult) -> Self {
        let (kind, values) = match result {
            NameLookupResult::Value(value) => (
                pb::resume_name_lookup::Kind::Value(value.root.0),
                Some(WireArena::new(value.graph)),
            ),
            NameLookupResult::Undefined => (pb::resume_name_lookup::Kind::Undefined(pb::Unit {}), None),
            NameLookupResult::Error(exc) => (pb::resume_name_lookup::Kind::Error((&exc).into()), None),
        };
        Self {
            values,
            kind: Some(kind),
        }
    }
}

impl TryFrom<pb::ResumeNameLookup> for NameLookupResult {
    type Error = ProtoConvertError;

    fn try_from(lookup: pb::ResumeNameLookup) -> Result<Self, ProtoConvertError> {
        let kind = lookup
            .kind
            .ok_or(ProtoConvertError::MissingField("ResumeNameLookup.kind"))?;
        match kind {
            pb::resume_name_lookup::Kind::Value(root) => Ok(Self::Value(value_from_parts(
                lookup.values,
                root,
                "ResumeNameLookup.values",
            )?)),
            pb::resume_name_lookup::Kind::Undefined(_) => Ok(Self::Undefined),
            pb::resume_name_lookup::Kind::Error(err) => Ok(Self::Error(MontyException::try_from(err)?)),
        }
    }
}

/// Projects settled futures onto the wire, merging every returned value into
/// the request's one arena.
#[must_use]
pub fn future_results_to_proto(results: Vec<(u32, ExtFunctionResult)>) -> pb::ResumeFutures {
    let mut values = MontyGraph::new();
    let results = results
        .into_iter()
        .map(|(call_id, result)| {
            let kind = match result {
                ExtFunctionResult::Return(value) => {
                    let offset = values.merge(value.graph);
                    pb::ext_function_result::Kind::ReturnValue(value.root.0 + offset)
                }
                ExtFunctionResult::Error(exc) => pb::ext_function_result::Kind::Error((&exc).into()),
                ExtFunctionResult::Future(id) => pb::ext_function_result::Kind::Future(id),
                ExtFunctionResult::NotFound(name) => pb::ext_function_result::Kind::NotFound(name),
            };
            pb::FutureResult {
                call_id,
                result: Some(pb::ExtFunctionResult { kind: Some(kind) }),
            }
        })
        .collect();
    pb::ResumeFutures {
        results,
        values: Some(WireArena::new(values)),
    }
}

/// Converts wire future results into `(call_id, result)` pairs for
/// `ResolveFutures::resume`; each returned value gets its own copy of the
/// nodes it reaches.
pub fn future_results_from_proto(
    results: Vec<pb::FutureResult>,
    values: Option<WireArena>,
) -> Result<Vec<(u32, ExtFunctionResult)>, ProtoConvertError> {
    let graph = arena_or_empty(values)?;
    results
        .into_iter()
        .map(|fr| {
            let result = fr
                .result
                .ok_or(ProtoConvertError::MissingField("FutureResult.result"))?;
            let kind = result
                .kind
                .ok_or(ProtoConvertError::MissingField("ExtFunctionResult.kind"))?;
            let result = match kind {
                pb::ext_function_result::Kind::ReturnValue(root) => {
                    let root = NodeId(root);
                    graph.check_root(root).map_err(|err| graph_error(&err))?;
                    ExtFunctionResult::Return(graph.value(root).to_owned())
                }
                other => ext_result_from_proto(pb::ExtFunctionResult { kind: Some(other) }, None)?,
            };
            Ok((fr.call_id, result))
        })
        .collect()
}

/// A single-value convenience for tests and hosts: the value of a
/// `ResumeCall` reply, as [`ext_result_from_proto`] reads it.
pub fn resume_call_result(call: pb::ResumeCall) -> Result<ExtFunctionResult, ProtoConvertError> {
    let result = call
        .result
        .ok_or(ProtoConvertError::MissingField("ResumeCall.result"))?;
    ext_result_from_proto(result, call.values)
}

impl From<MontyObject> for pb::ResumeCall {
    /// A `ResumeCall` returning `value`; the caller sets `call_id`.
    fn from(value: MontyObject) -> Self {
        let (result, values) = ext_result_to_proto(ExtFunctionResult::Return(value));
        Self {
            call_id: 0,
            result: Some(result),
            values,
        }
    }
}
