//! Conversions for resume payloads: the parent's answers to suspension
//! events (`ResumeCall`, `ResumeNameLookup`, `ResumeFutures`).

use monty::{ExtFunctionResult, MontyException, NameLookupResult};

use crate::{convert::ProtoConvertError, pb};

impl From<ExtFunctionResult> for pb::ExtResult {
    fn from(result: ExtFunctionResult) -> Self {
        let kind = match result {
            ExtFunctionResult::Return(obj) => pb::ext_result::Kind::ReturnValue(obj.into()),
            ExtFunctionResult::Error(exc) => pb::ext_result::Kind::Error((&exc).into()),
            ExtFunctionResult::Future(call_id) => pb::ext_result::Kind::Future(call_id),
            ExtFunctionResult::NotFound(name) => pb::ext_result::Kind::NotFound(name),
        };
        Self { kind: Some(kind) }
    }
}

impl TryFrom<pb::ExtResult> for ExtFunctionResult {
    type Error = ProtoConvertError;

    fn try_from(result: pb::ExtResult) -> Result<Self, ProtoConvertError> {
        let kind = result.kind.ok_or(ProtoConvertError::MissingField("ExtResult.kind"))?;
        match kind {
            pb::ext_result::Kind::ReturnValue(value) => Ok(Self::Return(value.into_object()?)),
            pb::ext_result::Kind::Error(err) => Ok(Self::Error(MontyException::try_from(err)?)),
            pb::ext_result::Kind::Future(call_id) => Ok(Self::Future(call_id)),
            pb::ext_result::Kind::NotFound(name) => Ok(Self::NotFound(name)),
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
            pb::resume_name_lookup::Kind::Value(value) => Ok(Self::Value(value.into_object()?)),
            pb::resume_name_lookup::Kind::Undefined(_) => Ok(Self::Undefined),
        }
    }
}

/// Converts wire future results into `(call_id, result)` pairs for
/// `ResolveFutures::resume`.
pub fn future_results_from_proto(
    results: Vec<pb::FutureResult>,
) -> Result<Vec<(u32, ExtFunctionResult)>, ProtoConvertError> {
    results
        .into_iter()
        .map(|fr| {
            let result = fr
                .result
                .ok_or(ProtoConvertError::MissingField("FutureResult.result"))?;
            Ok((fr.call_id, result.try_into()?))
        })
        .collect()
}
