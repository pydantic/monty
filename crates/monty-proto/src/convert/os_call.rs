//! Conversions for `OsCall` suspensions: the typed wire arms of
//! `monty.v1.OsCall` and [`OsFunctionCall`] map 1:1, so payloads (write data,
//! paths) *move* between the wire and the call — never clone. The one
//! value-typed argument (`Getenv.default`) indexes the message's arena.

use monty_types::{
    GetenvArgs, MkdirCallArgs, MontyPath, MontyTimeZone, OpenCallArgs, OsFunctionCall, PathBytesDataArgs,
    PathStringDataArgs, RenameCallArgs, UrandomArgs,
};

use crate::{
    convert::{ProtoConvertError, root_object},
    pb::{
        self, TimeZone, Unit,
        os_call::{self, Call},
    },
    wire::WireArena,
};

/// Builds the `OsCall` envelope: call id, typed arm and, for `Getenv`, the
/// arena its default indexes.
#[must_use]
pub fn os_call_to_proto(call_id: u32, call: OsFunctionCall) -> pb::OsCall {
    let (call, values) = call_to_proto(call);
    pb::OsCall {
        call_id,
        values,
        call: Some(call),
    }
}

/// Validates a decoded OS call envelope into its call id and typed call.
pub fn os_call_from_proto(call: pb::OsCall) -> Result<(u32, OsFunctionCall), ProtoConvertError> {
    let kind = call.call.ok_or(ProtoConvertError::MissingField("OsCall.call"))?;
    let function_call = match kind {
        os_call::Call::Getenv(g) => OsFunctionCall::Getenv(GetenvArgs {
            key: g.key,
            default: root_object(call.values, g.default, "OsCall.values")?,
        }),
        other => other.try_into()?,
    };
    Ok((call.call_id, function_call))
}

/// The typed wire arm of a call, with the arena `Getenv.default` indexes
/// (`None` for the value-free arms). Private so no caller can send a
/// `Getenv` without its arena.
fn call_to_proto(call: OsFunctionCall) -> (os_call::Call, Option<WireArena>) {
    let mut values = None;
    let call = match call {
        OsFunctionCall::Exists(p) => Call::Exists(p.into_string()),
        OsFunctionCall::IsFile(p) => Call::IsFile(p.into_string()),
        OsFunctionCall::IsDir(p) => Call::IsDir(p.into_string()),
        OsFunctionCall::IsSymlink(p) => Call::IsSymlink(p.into_string()),
        OsFunctionCall::ReadText(p) => Call::ReadText(p.into_string()),
        OsFunctionCall::ReadBytes(p) => Call::ReadBytes(p.into_string()),
        OsFunctionCall::Stat(p) => Call::Stat(p.into_string()),
        OsFunctionCall::Iterdir(p) => Call::Iterdir(p.into_string()),
        OsFunctionCall::Resolve(p) => Call::Resolve(p.into_string()),
        OsFunctionCall::Absolute(p) => Call::Absolute(p.into_string()),
        OsFunctionCall::Unlink(p) => Call::Unlink(p.into_string()),
        OsFunctionCall::Rmdir(p) => Call::Rmdir(p.into_string()),
        OsFunctionCall::WriteText(a) => Call::WriteText(text_write(a)),
        OsFunctionCall::AppendText(a) => Call::AppendText(text_write(a)),
        OsFunctionCall::WriteBytes(a) => Call::WriteBytes(bytes_write(a)),
        OsFunctionCall::AppendBytes(a) => Call::AppendBytes(bytes_write(a)),
        OsFunctionCall::Open(a) => Call::Open(os_call::Open {
            path: a.path.into_string(),
            mode: a.mode.as_str().to_owned(),
        }),
        OsFunctionCall::Mkdir(a) => Call::Mkdir(os_call::Mkdir {
            path: a.path.into_string(),
            parents: a.parents,
            exist_ok: a.exist_ok,
        }),
        OsFunctionCall::Rename(a) => Call::Rename(os_call::Rename {
            src: a.src.into_string(),
            dst: a.dst.into_string(),
        }),
        OsFunctionCall::Getenv(a) => {
            values = Some(WireArena::new(a.default.graph));
            Call::Getenv(os_call::Getenv {
                key: a.key,
                default: a.default.root.0,
            })
        }
        OsFunctionCall::GetEnviron => Call::GetEnviron(Unit {}),
        OsFunctionCall::DateToday => Call::DateToday(Unit {}),
        OsFunctionCall::DateTimeNow(tz) => Call::DateTimeNow(os_call::DateTimeNow {
            tz: tz.map(|tz| TimeZone {
                offset_seconds: tz.offset_seconds,
                name: tz.name,
            }),
        }),
        OsFunctionCall::Urandom(a) => Call::Urandom(os_call::Urandom { size: a.size }),
    };
    (call, values)
}

/// The value-free arms; `Getenv` needs the envelope's arena, see
/// [`os_call_from_proto`].
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
            os_call::Call::Getenv(_) => {
                return Err(ProtoConvertError::InvalidValue {
                    field: "OsCall.getenv",
                    reason: "getenv carries a value and must be converted with its arena".to_owned(),
                });
            }
            os_call::Call::GetEnviron(_) => Self::GetEnviron,
            os_call::Call::DateToday(_) => Self::DateToday,
            // typed arm: the wire cannot express anything but an optional
            // timezone here, mirroring the VM's validation of `datetime.now`
            os_call::Call::DateTimeNow(now) => Self::DateTimeNow(now.tz.map(|tz| MontyTimeZone {
                offset_seconds: tz.offset_seconds,
                name: tz.name,
            })),
            os_call::Call::Urandom(u) => Self::Urandom(UrandomArgs { size: u.size }),
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
