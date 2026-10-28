//! Converts semantic component requests into protocol requests.

use monty_proto::{BudgetVec, PROTOCOL_VERSION, WireArena, pb};
use monty_types::MONTY_VERSION;

use crate::{
    types::{
        CallResult, ConfigureRequest, DatetimeSource, NameLookupResult, OsPolicy, ProcessTime, RaisedError, RandomSeed,
        RandomStart, Request, SleepMode, TimeZone, TypeCheckFormat,
    },
    value,
};

/// Converts a semantic component request into the protocol's request type.
/// A request's arena is validated here; the roots the request names are
/// checked by the child, like any wire frame's.
pub fn request_from_component(request: Request) -> Result<pb::ParentRequest, String> {
    let mut budget = value::DecodeBudget::default();
    let kind = match request {
        Request::Configure(request) => pb::parent_request::Kind::Configure(configure_from_component(request)),
        Request::Feed(request) => pb::parent_request::Kind::Feed(pb::Feed {
            code: request.code,
            inputs: request
                .inputs
                .into_iter()
                .map(|input| pb::NamedRef {
                    name: input.name,
                    value: input.value,
                })
                .collect(),
            values: Some(WireArena::new(value::from_component(request.values, &mut budget)?)),
            skip_type_check: request.skip_type_check,
            cwd: request.cwd,
        }),
        Request::ResumeCall(request) => pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
            call_id: request.call_id,
            result: Some(call_result_from_component(request.outcome)),
            values: Some(WireArena::new(value::from_component(request.values, &mut budget)?)),
        }),
        Request::ResumeNameLookup(request) => {
            let kind = match request.outcome {
                NameLookupResult::Value(root) => pb::resume_name_lookup::Kind::Value(root),
                NameLookupResult::Undefined => pb::resume_name_lookup::Kind::Undefined(pb::Unit {}),
                NameLookupResult::Error(error) => {
                    pb::resume_name_lookup::Kind::Error(raised_exception_from_component(error))
                }
            };
            pb::parent_request::Kind::ResumeNameLookup(pb::ResumeNameLookup {
                values: Some(WireArena::new(value::from_component(request.values, &mut budget)?)),
                kind: Some(kind),
            })
        }
        Request::ResumeFutures(request) => pb::parent_request::Kind::ResumeFutures(pb::ResumeFutures {
            results: request
                .results
                .into_iter()
                .map(|result| pb::FutureResult {
                    call_id: result.call_id,
                    result: Some(call_result_from_component(result.outcome)),
                })
                .collect(),
            values: Some(WireArena::new(value::from_component(request.values, &mut budget)?)),
        }),
        Request::AbortFeed(error) => pb::parent_request::Kind::AbortFeed(pb::AbortFeed {
            exception: Some(raised_exception_from_component(error)),
        }),
        Request::Dump => pb::parent_request::Kind::Dump(pb::Dump {}),
        Request::Load(state) => pb::parent_request::Kind::Load(pb::Load { state: state.into() }),
        Request::Reset => pb::parent_request::Kind::Reset(pb::Reset {}),
        Request::InstallDependencies(requirements) => {
            pb::parent_request::Kind::InstallDependencies(pb::InstallDependencies {
                requirements: requirements.into(),
            })
        }
    };
    Ok(pb::ParentRequest {
        kind: Some(kind),
        trace_parent: None,
    })
}

/// Converts session options into the protocol child's configuration type.
fn configure_from_component(request: ConfigureRequest) -> pb::Configure {
    let persistence = match request.ephemeral {
        None => pb::Persistence::Unspecified,
        Some(true) => pb::Persistence::Ephemeral,
        Some(false) => pb::Persistence::Stored,
    };
    pb::Configure {
        script_name: request.script_name,
        limits: request.limits.map(|limits| pb::ResourceLimits {
            max_feed_duration_micros: limits.max_feed_duration_micros,
            max_turn_duration_micros: limits.max_turn_duration_micros,
            max_memory_bytes: limits.max_memory_bytes,
            gc_interval: limits.gc_interval,
            max_recursion_depth: limits.max_recursion_depth,
            max_suspensions: limits.max_suspensions,
            max_total_sleep_micros: limits.max_total_sleep_micros,
        }),
        type_check: request.type_check,
        type_check_stubs: request.type_check_stubs,
        monty_version: MONTY_VERSION.to_owned(),
        assert_message_annotations: request.assert_message_annotations,
        type_check_format: i32::from(type_check_format_from_component(request.type_check_format)),
        type_check_color: request.type_check_color,
        protocol_version: PROTOCOL_VERSION,
        // The in-process component batches frames until the end of a turn, but
        // their boundaries survive it: the host gets one print callback per
        // frame, and a print collector charges its cap per frame.
        print_flush_interval_ms: request.print_flush_interval_ms,
        os_policy: request.os_policy.map(os_policy_from_component),
        // a relay's concern; a local child never stores sessions or has profiles
        persistence: persistence.into(),
        profile: request.profile,
    }
}

/// Protocol conversion validates these component settings as untrusted parent input.
fn os_policy_from_component(calls: OsPolicy) -> pb::OsPolicy {
    let datetime = calls.datetime.map(|source| match source {
        DatetimeSource::System => pb::os_policy::Datetime::System(pb::Unit {}),
        DatetimeSource::CallHost => pb::os_policy::Datetime::CallHost(pb::Unit {}),
        DatetimeSource::Fixed(fixed) => pb::os_policy::Datetime::Fixed(pb::FixedDateTime {
            unix_seconds: fixed.unix_seconds,
            microsecond: fixed.microsecond,
        }),
    });
    let timezone = calls.timezone.map(|zone| pb::SandboxTimeZone {
        zone: Some(match zone {
            TimeZone::Utc => pb::sandbox_time_zone::Zone::Utc(pb::Unit {}),
            TimeZone::Named(name) => pb::sandbox_time_zone::Zone::Named(name),
            TimeZone::Fixed(fixed) => pb::sandbox_time_zone::Zone::Fixed(pb::TimeZone {
                offset_seconds: fixed.offset_seconds,
                name: fixed.name,
            }),
        }),
    });
    let sleep = calls.sleep.map(|mode| pb::SleepMode {
        mode: Some(match mode {
            SleepMode::System(max_micros) => pb::sleep_mode::Mode::System(pb::SystemSleep { max_micros }),
            SleepMode::CallHost => pb::sleep_mode::Mode::CallHost(pb::Unit {}),
            SleepMode::Zero => pb::sleep_mode::Mode::Zero(pb::Unit {}),
        }),
    });
    let random_start = calls.random_start.map(|start| match start {
        RandomStart::System => pb::os_policy::RandomStart::RandomSystem(pb::Unit {}),
        RandomStart::CallHost => pb::os_policy::RandomStart::RandomCallHost(pb::Unit {}),
        RandomStart::Seed(seed) => pb::os_policy::RandomStart::Seed(pb::RandomSeed {
            value: Some(match seed {
                RandomSeed::Int(bytes) => pb::random_seed::Value::Int(bytes.into()),
                RandomSeed::Float(f) => pb::random_seed::Value::Float(f),
                RandomSeed::Str(s) => pb::random_seed::Value::Str(s),
                RandomSeed::Bytes(b) => pb::random_seed::Value::Bytes(b.into()),
            }),
        }),
    });
    let process_time = calls.process_time.map(|source| match source {
        ProcessTime::Zero => pb::os_policy::ProcessTime::Zero(pb::Unit {}),
        ProcessTime::Elapsed => pb::os_policy::ProcessTime::Elapsed(pb::Unit {}),
    });
    pb::OsPolicy {
        datetime,
        timezone,
        sleep,
        process_time,
        random_start,
    }
}

/// Converts the component's diagnostic format into the protocol enum.
fn type_check_format_from_component(format: TypeCheckFormat) -> pb::TypeCheckFormat {
    match format {
        TypeCheckFormat::Full => pb::TypeCheckFormat::Full,
        TypeCheckFormat::Concise => pb::TypeCheckFormat::Concise,
        TypeCheckFormat::Azure => pb::TypeCheckFormat::Azure,
        TypeCheckFormat::Json => pb::TypeCheckFormat::Json,
        TypeCheckFormat::JsonLines => pb::TypeCheckFormat::JsonLines,
        TypeCheckFormat::Rdjson => pb::TypeCheckFormat::Rdjson,
        TypeCheckFormat::Pylint => pb::TypeCheckFormat::Pylint,
        TypeCheckFormat::Gitlab => pb::TypeCheckFormat::Gitlab,
        TypeCheckFormat::Github => pb::TypeCheckFormat::Github,
    }
}

/// Converts a host call outcome into the child state machine's result type;
/// a returned value stays the index into the request's arena.
fn call_result_from_component(result: CallResult) -> pb::ExtFunctionResult {
    let kind = match result {
        CallResult::ReturnValue(root) => pb::ext_function_result::Kind::ReturnValue(root),
        CallResult::Error(error) => pb::ext_function_result::Kind::Error(raised_exception_from_component(error)),
        CallResult::PendingFuture(call_id) => pb::ext_function_result::Kind::Future(call_id),
        CallResult::NotFound(name) => pb::ext_function_result::Kind::NotFound(name),
        CallResult::NotHandled => pb::ext_function_result::Kind::NotHandled(pb::Unit {}),
    };
    pb::ExtFunctionResult { kind: Some(kind) }
}

/// Converts a host-raised error into the protocol's exception message; the
/// host supplies no traceback or structured data.
fn raised_exception_from_component(error: RaisedError) -> pb::RaisedException {
    pb::RaisedException {
        exc_type: error.exc_type,
        message: Some(error.message),
        traceback: BudgetVec::new(),
        data: None,
    }
}
