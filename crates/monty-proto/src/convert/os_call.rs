//! Conversions for `OsCall` suspensions: the typed wire arms of
//! `monty.v1.OsCall` and [`OsFunctionCall`] map 1:1, so payloads (write data,
//! paths) *move* between the wire and the call — never clone.
//!
//! [`os_call::Call::Consumed`] is the one arm with no monty equivalent: it
//! re-announces a call restored from a dump whose argument payload was
//! consumed when the call was first announced. Receivers must match it before
//! converting; `TryFrom` rejects it defensively.

use monty::{
    GetenvArgs, MkdirCallArgs, MontyPath, MontyTimeZone, OpenCallArgs, OsFunctionCall, PathBytesDataArgs,
    PathStringDataArgs, RenameCallArgs,
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
            OsFunctionCall::Used => unreachable!("OsFunctionCall::Used encoded after take_function_call"),
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
            // a consumed re-announcement carries no call — callers surface it
            // to their fallback handler instead of converting it
            os_call::Call::Consumed(_) => {
                return Err(ProtoConvertError::InvalidValue {
                    field: "OsCall.call",
                    reason: "consumed re-announcement carries no call".to_owned(),
                });
            }
        })
    }
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
