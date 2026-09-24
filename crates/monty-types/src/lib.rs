#![doc = include_str!("../README.md")]

/// The monty version this build was compiled as.
pub const MONTY_VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod args;
mod builtins;
mod exceptions;
mod file_mode;
pub mod format;
mod graph;
mod io;
mod object;
mod os;
mod os_policy;
mod resource;
mod results;
mod run_options;
mod type_checking;
mod uuid;
mod virtual_path;

pub use crate::{
    builtins::BuiltinsFunctions,
    exceptions::{
        CodeLoc, ExcData, ExcType, JsonErrorData, MontyException, SourceRange, StackFrame, UnicodeErrorData,
        UnicodeErrorObject, unicode_decode_error_msg,
    },
    file_mode::FileMode,
    format::{FormatFloat, StringRepr, bytes_repr, bytes_repr_fmt, string_repr_fmt, utf8_error_reason},
    io::{
        COLLECT_STREAMS_ENTRY_OVERHEAD, CollectedStreams, DEFAULT_MAX_PRINT_COLLECT_BYTES, PrintStream, PrintWriter,
        PrintWriterCallback, check_print_collect_limit,
    },
    object::{
        CallArgs, ConversionError, InvalidInputError, MAX_TIMEZONE_OFFSET_SECONDS, MIN_TIMEZONE_OFFSET_SECONDS,
        MontyDate, MontyDateTime, MontyFileHandle, MontyObject, MontyTime, MontyTimeDelta, MontyTimeZone, MontyType,
        NamedValues, ObjectRef, unstable,
    },
    os::{
        GetenvArgs, MAX_SLEEP_SECONDS, MkdirCallArgs, MontyPath, OpenCallArgs, OsFunctionCall, PathBytesDataArgs,
        PathStringDataArgs, RenameCallArgs, SleepError, TimeCaller, UrandomArgs, dir_stat, file_stat, sleep_duration,
        sleep_duration_saturating, stat_result, symlink_stat,
    },
    os_policy::{
        DateTimeSource, NamedZone, OsPolicy, ProcessTime, RandomSeed, RandomStart, SandboxTimeZone, SleepMode,
        UnknownTimeZone, ZoneConstants, local_wall_clock, unix_seconds,
    },
    resource::{
        BASELINE_MEMORY, DEFAULT_MAX_RECURSION_DEPTH, DEFAULT_MAX_SUSPENSIONS, LARGE_RESULT_THRESHOLD, LIVE_MEMORY,
        OOM_EXIT_CODE, ResourceError, ResourceLimits, ResourceTracker, TimeLimitScope, memory_limit_with_headroom,
    },
    results::{ExtFunctionResult, NameLookupResult},
    run_options::{AssertMessageAnnotations, CompileOptions, SOURCE_SCAN_THRESHOLD},
    type_checking::{TypeCheckState, TypeCheckingConfig, TypeCheckingFormat},
    uuid::MontyUuid,
    virtual_path::{normalize_virtual_path, validate_cwd},
};
