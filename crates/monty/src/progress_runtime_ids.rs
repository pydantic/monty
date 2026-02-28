//! Shared helpers for exposing runtime-ID slices from progress enums.

use crate::{object::MontyObject, runtime_id::RuntimeValueId};

pub(crate) type RuntimeIdSlices<'a> = (&'a [RuntimeValueId], &'a [(RuntimeValueId, RuntimeValueId)]);

pub(crate) struct CheckedRuntimeIdPayload {
    pub(crate) args: Vec<MontyObject>,
    pub(crate) arg_runtime_ids: Vec<RuntimeValueId>,
    pub(crate) kwargs: Vec<(MontyObject, MontyObject)>,
    pub(crate) kwarg_runtime_ids: Vec<(RuntimeValueId, RuntimeValueId)>,
}

#[expect(
    clippy::struct_field_names,
    reason = "Field names mirror validated payload names and are explicitly required by the API contract"
)]
pub(crate) struct RuntimeIdCardinality {
    pub args_len: usize,
    pub arg_runtime_ids_len: usize,
    pub kwargs_len: usize,
    pub kwarg_runtime_ids_len: usize,
}

impl RuntimeIdCardinality {
    pub fn new(args_len: usize, arg_runtime_ids_len: usize, kwargs_len: usize, kwarg_runtime_ids_len: usize) -> Self {
        Self {
            args_len,
            arg_runtime_ids_len,
            kwargs_len,
            kwarg_runtime_ids_len,
        }
    }
}

pub(crate) fn validate_runtime_id_cardinality(context: &str, cardinality: &RuntimeIdCardinality) -> Result<(), String> {
    if cardinality.arg_runtime_ids_len != cardinality.args_len {
        return Err(format!(
            "{context} payload is malformed: arg_runtime_ids length ({}) does not match args length ({})",
            cardinality.arg_runtime_ids_len, cardinality.args_len
        ));
    }

    if cardinality.kwarg_runtime_ids_len != cardinality.kwargs_len {
        return Err(format!(
            "{context} payload is malformed: kwarg_runtime_ids length ({}) does not match kwargs length ({})",
            cardinality.kwarg_runtime_ids_len, cardinality.kwargs_len
        ));
    }

    Ok(())
}

pub(crate) fn checked_runtime_id_payload(
    args: Vec<MontyObject>,
    arg_runtime_ids: Vec<RuntimeValueId>,
    kwargs: Vec<(MontyObject, MontyObject)>,
    kwarg_runtime_ids: Vec<(RuntimeValueId, RuntimeValueId)>,
) -> CheckedRuntimeIdPayload {
    CheckedRuntimeIdPayload {
        args,
        arg_runtime_ids,
        kwargs,
        kwarg_runtime_ids,
    }
}

macro_rules! progress_runtime_ids {
    ($progress:expr) => {
        match $progress {
            Self::FunctionCall {
                arg_runtime_ids,
                kwarg_runtime_ids,
                ..
            }
            | Self::OsCall {
                arg_runtime_ids,
                kwarg_runtime_ids,
                ..
            } => Some((arg_runtime_ids.as_slice(), kwarg_runtime_ids.as_slice())),
            _ => None,
        }
    };
}

pub(crate) use progress_runtime_ids;
