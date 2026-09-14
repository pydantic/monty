//! Conversions for `OsCall` suspensions: the typed wire arms of
//! `monty.v1.OsCall` and [`OsFunctionCall`] map 1:1, so payloads (write data,
//! paths) *move* between the wire and the call — never clone.

use std::time::Duration;

use monty_types::{
    AsyncSleepArgs, GetenvArgs, MkdirCallArgs, MontyPath, MontyTimeZone, OpenCallArgs, OsFunctionCall,
    PathBytesDataArgs, PathStringDataArgs, RenameCallArgs, sleep_duration,
};

use crate::{
    convert::ProtoConvertError,
    pb::{TimeZone, Unit, os_call},
};

impl From<OsFunctionCall> for os_call::Call {
    fn from(call: OsFunctionCall) -> Self {
        match call {
            OsFunctionCall::Exists(p) => Self::Exists(p.into_string()),
            OsFunctionCall::IsFile(p) => Self::IsFile(p.into_string()),
            OsFunctionCall::IsDir(p) => Self::IsDir(p.into_string()),
            OsFunctionCall::IsSymlink(p) => Self::IsSymlink(p.into_string()),
            OsFunctionCall::ReadText(p) => Self::ReadText(p.into_string()),
            OsFunctionCall::ReadBytes(p) => Self::ReadBytes(p.into_string()),
            OsFunctionCall::Stat(p) => Self::Stat(p.into_string()),
            OsFunctionCall::Iterdir(p) => Self::Iterdir(p.into_string()),
            OsFunctionCall::Resolve(p) => Self::Resolve(p.into_string()),
            OsFunctionCall::Absolute(p) => Self::Absolute(p.into_string()),
            OsFunctionCall::Unlink(p) => Self::Unlink(p.into_string()),
            OsFunctionCall::Rmdir(p) => Self::Rmdir(p.into_string()),
            OsFunctionCall::WriteText(a) => Self::WriteText(text_write(a)),
            OsFunctionCall::AppendText(a) => Self::AppendText(text_write(a)),
            OsFunctionCall::WriteBytes(a) => Self::WriteBytes(bytes_write(a)),
            OsFunctionCall::AppendBytes(a) => Self::AppendBytes(bytes_write(a)),
            OsFunctionCall::Open(a) => Self::Open(os_call::Open {
                path: a.path.into_string(),
                mode: a.mode.as_str().to_owned(),
            }),
            OsFunctionCall::Mkdir(a) => Self::Mkdir(os_call::Mkdir {
                path: a.path.into_string(),
                parents: a.parents,
                exist_ok: a.exist_ok,
            }),
            OsFunctionCall::Rename(a) => Self::Rename(os_call::Rename {
                src: a.src.into_string(),
                dst: a.dst.into_string(),
            }),
            OsFunctionCall::Getenv(a) => Self::Getenv(os_call::Getenv {
                key: a.key,
                default: Some(a.default.into()),
            }),
            OsFunctionCall::GetEnviron => Self::GetEnviron(Unit {}),
            OsFunctionCall::DateToday => Self::DateToday(Unit {}),
            OsFunctionCall::DateTimeNow(tz) => Self::DateTimeNow(os_call::DateTimeNow {
                tz: tz.map(|tz| TimeZone {
                    offset_seconds: tz.offset_seconds,
                    name: tz.name,
                }),
            }),
            OsFunctionCall::Time => Self::Time(Unit {}),
            OsFunctionCall::Sleep(delay) => Self::Sleep(os_call::Sleep {
                seconds: delay.as_secs_f64(),
            }),
            OsFunctionCall::AsyncSleep(a) => Self::AsyncSleep(os_call::AsyncSleep {
                delay: a.delay.as_secs_f64(),
                result: Some(a.result.into()),
            }),
        }
    }
}

impl TryFrom<os_call::Call> for OsFunctionCall {
    type Error = ProtoConvertError;

    fn try_from(call: os_call::Call) -> Result<Self, ProtoConvertError> {
        Ok(match call {
            os_call::Call::Exists(p) => Self::Exists(MontyPath::new(p)),
            os_call::Call::IsFile(p) => Self::IsFile(MontyPath::new(p)),
            os_call::Call::IsDir(p) => Self::IsDir(MontyPath::new(p)),
            os_call::Call::IsSymlink(p) => Self::IsSymlink(MontyPath::new(p)),
            os_call::Call::ReadText(p) => Self::ReadText(MontyPath::new(p)),
            os_call::Call::ReadBytes(p) => Self::ReadBytes(MontyPath::new(p)),
            os_call::Call::Stat(p) => Self::Stat(MontyPath::new(p)),
            os_call::Call::Iterdir(p) => Self::Iterdir(MontyPath::new(p)),
            os_call::Call::Resolve(p) => Self::Resolve(MontyPath::new(p)),
            os_call::Call::Absolute(p) => Self::Absolute(MontyPath::new(p)),
            os_call::Call::Unlink(p) => Self::Unlink(MontyPath::new(p)),
            os_call::Call::Rmdir(p) => Self::Rmdir(MontyPath::new(p)),
            os_call::Call::WriteText(a) => Self::WriteText(text_args(a)),
            os_call::Call::AppendText(a) => Self::AppendText(text_args(a)),
            os_call::Call::WriteBytes(a) => Self::WriteBytes(bytes_args(a)),
            os_call::Call::AppendBytes(a) => Self::AppendBytes(bytes_args(a)),
            os_call::Call::Open(o) => Self::Open(OpenCallArgs {
                mode: o.mode.parse().map_err(|_| ProtoConvertError::InvalidFileMode(o.mode))?,
                path: MontyPath::new(o.path),
            }),
            os_call::Call::Mkdir(m) => Self::Mkdir(MkdirCallArgs {
                path: MontyPath::new(m.path),
                parents: m.parents,
                exist_ok: m.exist_ok,
            }),
            os_call::Call::Rename(r) => Self::Rename(RenameCallArgs {
                src: MontyPath::new(r.src),
                dst: MontyPath::new(r.dst),
            }),
            os_call::Call::Getenv(g) => Self::Getenv(GetenvArgs {
                key: g.key,
                default: g
                    .default
                    .ok_or(ProtoConvertError::MissingField("Getenv.default"))?
                    .into_object()?,
            }),
            os_call::Call::GetEnviron(_) => Self::GetEnviron,
            os_call::Call::DateToday(_) => Self::DateToday,
            // typed arm: the wire cannot express anything but an optional
            // timezone here, mirroring the VM's validation of `datetime.now`
            os_call::Call::DateTimeNow(now) => Self::DateTimeNow(now.tz.map(|tz| MontyTimeZone {
                offset_seconds: tz.offset_seconds,
                name: tz.name,
            })),
            os_call::Call::Time(_) => Self::Time,
            os_call::Call::Sleep(s) => Self::Sleep(delay(s.seconds, "Sleep.seconds")?),
            os_call::Call::AsyncSleep(s) => Self::AsyncSleep(AsyncSleepArgs {
                delay: delay(s.delay, "AsyncSleep.delay")?,
                result: s
                    .result
                    .ok_or(ProtoConvertError::MissingField("AsyncSleep.result"))?
                    .into_object()?,
            }),
        })
    }
}

/// Validates wire seconds into a `Duration`.
///
/// A child may be compromised, so a NaN, negative or unrepresentable delay is
/// refused here rather than reaching a host that would convert it — and panic
/// doing so.
fn delay(seconds: f64, field: &'static str) -> Result<Duration, ProtoConvertError> {
    sleep_duration(seconds).map_err(|_| ProtoConvertError::InvalidValue {
        field,
        reason: format!("sleep length {seconds} is not a finite, non-negative number of seconds"),
    })
}

/// `PathStringDataArgs` → wire `TextWrite`, moving the text payload.
fn text_write(args: PathStringDataArgs) -> os_call::TextWrite {
    os_call::TextWrite {
        path: args.path.into_string(),
        data: args.data,
    }
}

/// `PathBytesDataArgs` → wire `BytesWrite`, moving the bytes payload.
fn bytes_write(args: PathBytesDataArgs) -> os_call::BytesWrite {
    os_call::BytesWrite {
        path: args.path.into_string(),
        data: args.data,
    }
}

/// Wire `TextWrite` → `PathStringDataArgs`, moving the text payload.
fn text_args(wire: os_call::TextWrite) -> PathStringDataArgs {
    PathStringDataArgs {
        path: MontyPath::new(wire.path),
        data: wire.data,
    }
}

/// Wire `BytesWrite` → `PathBytesDataArgs`, moving the bytes payload.
fn bytes_args(wire: os_call::BytesWrite) -> PathBytesDataArgs {
    PathBytesDataArgs {
        path: MontyPath::new(wire.path),
        data: wire.data,
    }
}
