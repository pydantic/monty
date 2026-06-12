//! `MontyObject` ↔ `pb::MontyValue` conversions — the core of the value
//! protocol. Every `MontyObject` variant maps to exactly one `MontyValue`
//! oneof arm; see the `.proto` for the representation choices (BigInt as
//! sign+magnitude, dicts as ordered pairs, enums as display-name strings).

use std::ops::RangeInclusive;

use monty::{DictPairs, MontyDate, MontyDateTime, MontyFileHandle, MontyObject, MontyTimeDelta, MontyTimeZone, Type};
use num_bigint::{BigInt, Sign};

use crate::{
    convert::{ProtoConvertError, pairs_from_proto, values_from_proto, values_to_proto},
    pb,
    pb::monty_value::Kind,
};

impl From<&MontyObject> for pb::MontyValue {
    fn from(obj: &MontyObject) -> Self {
        let kind = match obj {
            MontyObject::Ellipsis => Kind::Ellipsis(pb::Unit {}),
            MontyObject::None => Kind::None(pb::Unit {}),
            MontyObject::Bool(b) => Kind::Boolean(*b),
            MontyObject::Int(i) => Kind::Int(*i),
            MontyObject::BigInt(bi) => Kind::Bigint(bigint_to_proto(bi)),
            MontyObject::Float(f) => Kind::Float(*f),
            MontyObject::String(s) => Kind::Str(s.clone()),
            MontyObject::Bytes(b) => Kind::Bytes(b.clone()),
            MontyObject::List(items) => Kind::List(value_list(items)),
            MontyObject::Tuple(items) => Kind::Tuple(value_list(items)),
            MontyObject::NamedTuple {
                type_name,
                field_names,
                values,
            } => Kind::NamedTuple(pb::NamedTupleValue {
                type_name: type_name.clone(),
                field_names: field_names.clone(),
                values: values_to_proto(values),
            }),
            MontyObject::Dict(pairs) => Kind::Dict(dict_to_proto(pairs)),
            MontyObject::Set(items) => Kind::Set(value_list(items)),
            MontyObject::FrozenSet(items) => Kind::FrozenSet(value_list(items)),
            MontyObject::Date(d) => Kind::Date(pb::DateValue {
                year: d.year,
                month: u32::from(d.month),
                day: u32::from(d.day),
            }),
            MontyObject::DateTime(dt) => Kind::Datetime(pb::DateTimeValue {
                year: dt.year,
                month: u32::from(dt.month),
                day: u32::from(dt.day),
                hour: u32::from(dt.hour),
                minute: u32::from(dt.minute),
                second: u32::from(dt.second),
                microsecond: dt.microsecond,
                offset_seconds: dt.offset_seconds,
                timezone_name: dt.timezone_name.clone(),
            }),
            MontyObject::TimeDelta(td) => Kind::Timedelta(pb::TimeDeltaValue {
                days: td.days,
                seconds: td.seconds,
                microseconds: td.microseconds,
            }),
            MontyObject::TimeZone(tz) => Kind::Timezone(pb::TimeZoneValue {
                offset_seconds: tz.offset_seconds,
                name: tz.name.clone(),
            }),
            MontyObject::Exception { exc_type, arg } => Kind::Exception(pb::ExceptionValue {
                exc_type: exc_type.to_string(),
                arg: arg.clone(),
            }),
            MontyObject::Type(t) => Kind::Type(t.to_string()),
            MontyObject::BuiltinFunction(bf) => Kind::BuiltinFunction(bf.to_string()),
            MontyObject::Path(p) => Kind::Path(p.clone()),
            MontyObject::FileHandle(fh) => Kind::FileHandle(pb::FileHandleValue {
                path: fh.path.clone(),
                mode: fh.mode.as_str().to_owned(),
                position: fh.position,
            }),
            MontyObject::Dataclass {
                name,
                type_id,
                field_names,
                attrs,
                frozen,
            } => Kind::Dataclass(pb::DataclassValue {
                name: name.clone(),
                type_id: *type_id,
                field_names: field_names.clone(),
                attrs: Some(dict_to_proto(attrs)),
                frozen: *frozen,
            }),
            MontyObject::Function { name, docstring } => Kind::Function(pb::FunctionValue {
                name: name.clone(),
                docstring: docstring.clone(),
            }),
            MontyObject::Repr(r) => Kind::Repr(r.clone()),
            MontyObject::Cycle(heap_id, placeholder) => Kind::Cycle(pb::CycleValue {
                heap_id: heap_id.index() as u64,
                placeholder: placeholder.clone(),
            }),
        };
        Self { kind: Some(kind) }
    }
}

impl TryFrom<pb::MontyValue> for MontyObject {
    type Error = ProtoConvertError;

    fn try_from(value: pb::MontyValue) -> Result<Self, ProtoConvertError> {
        let kind = value.kind.ok_or(ProtoConvertError::MissingField("MontyValue.kind"))?;
        match kind {
            Kind::Ellipsis(_) => Ok(Self::Ellipsis),
            Kind::None(_) => Ok(Self::None),
            Kind::Boolean(b) => Ok(Self::Bool(b)),
            Kind::Int(i) => Ok(Self::Int(i)),
            Kind::Bigint(bi) => Ok(Self::BigInt(bigint_from_proto(&bi))),
            Kind::Float(f) => Ok(Self::Float(f)),
            Kind::Str(s) => Ok(Self::String(s)),
            Kind::Bytes(b) => Ok(Self::Bytes(b)),
            Kind::List(items) => Ok(Self::List(values_from_proto(items.items)?)),
            Kind::Tuple(items) => Ok(Self::Tuple(values_from_proto(items.items)?)),
            Kind::NamedTuple(nt) => Ok(Self::NamedTuple {
                type_name: nt.type_name,
                field_names: nt.field_names,
                values: values_from_proto(nt.values)?,
            }),
            Kind::Dict(dict) => Ok(Self::Dict(dict_from_proto(dict)?)),
            Kind::Set(items) => Ok(Self::Set(values_from_proto(items.items)?)),
            Kind::FrozenSet(items) => Ok(Self::FrozenSet(values_from_proto(items.items)?)),
            Kind::Date(d) => {
                let (year, month, day) = date_fields(
                    d.year,
                    d.month,
                    d.day,
                    ["DateValue.year", "DateValue.month", "DateValue.day"],
                )?;
                Ok(Self::Date(MontyDate { year, month, day }))
            }
            Kind::Datetime(dt) => {
                if dt.offset_seconds.is_none() && dt.timezone_name.is_some() {
                    return Err(ProtoConvertError::InvalidValue {
                        field: "DateTimeValue.timezone_name",
                        reason: "timezone_name requires offset_seconds".to_owned(),
                    });
                }
                let (year, month, day) = date_fields(
                    dt.year,
                    dt.month,
                    dt.day,
                    ["DateTimeValue.year", "DateTimeValue.month", "DateTimeValue.day"],
                )?;
                Ok(Self::DateTime(MontyDateTime {
                    year,
                    month,
                    day,
                    hour: ranged_u8(dt.hour, 0..=23, "DateTimeValue.hour")?,
                    minute: ranged_u8(dt.minute, 0..=59, "DateTimeValue.minute")?,
                    second: ranged_u8(dt.second, 0..=59, "DateTimeValue.second")?,
                    microsecond: bounded(dt.microsecond, 999_999, "DateTimeValue.microsecond")?,
                    offset_seconds: dt.offset_seconds,
                    timezone_name: dt.timezone_name,
                }))
            }
            Kind::Timedelta(td) => Ok(Self::TimeDelta(MontyTimeDelta {
                days: td.days,
                // out-of-range components would violate `MontyTimeDelta`'s
                // documented normalization invariants and corrupt arithmetic
                // and formatting once inside the sandbox
                seconds: normalized(td.seconds, 86_400, "TimeDeltaValue.seconds")?,
                microseconds: normalized(td.microseconds, 1_000_000, "TimeDeltaValue.microseconds")?,
            })),
            Kind::Timezone(tz) => Ok(Self::TimeZone(MontyTimeZone {
                offset_seconds: tz.offset_seconds,
                name: tz.name,
            })),
            Kind::Exception(exc) => Ok(Self::Exception {
                exc_type: exc
                    .exc_type
                    .parse()
                    .map_err(|_| ProtoConvertError::UnknownExcType(exc.exc_type))?,
                arg: exc.arg,
            }),
            Kind::Type(name) => Type::from_type_name(&name)
                .map(Self::Type)
                .ok_or(ProtoConvertError::UnknownType(name)),
            Kind::BuiltinFunction(name) => {
                Self::builtin_function_from_name(&name).ok_or(ProtoConvertError::UnknownBuiltinFunction(name))
            }
            Kind::Path(p) => Ok(Self::Path(p)),
            Kind::FileHandle(fh) => Ok(Self::FileHandle(MontyFileHandle {
                mode: fh
                    .mode
                    .parse()
                    .map_err(|_| ProtoConvertError::InvalidFileMode(fh.mode))?,
                path: fh.path,
                position: fh.position,
            })),
            Kind::Dataclass(dc) => Ok(Self::Dataclass {
                name: dc.name,
                type_id: dc.type_id,
                field_names: dc.field_names,
                attrs: dict_from_proto(
                    dc.attrs
                        .ok_or(ProtoConvertError::MissingField("DataclassValue.attrs"))?,
                )?,
                frozen: dc.frozen,
            }),
            Kind::Function(func) => Ok(Self::Function {
                name: func.name,
                docstring: func.docstring,
            }),
            // `Repr` round-trips so values can be logged/echoed; using one as
            // an *execution input* is rejected later by `MontyObject::to_value`
            // with a proper Python-level error.
            Kind::Repr(r) => Ok(Self::Repr(r)),
            // A heap id is meaningless outside the process that produced it,
            // and `HeapId` deliberately has no public constructor.
            Kind::Cycle(_) => Err(ProtoConvertError::OutputOnly("cycle")),
        }
    }
}

/// Encodes a `BigInt` as sign + big-endian magnitude.
fn bigint_to_proto(bi: &BigInt) -> pb::BigIntValue {
    let (sign, magnitude) = bi.to_bytes_be();
    pb::BigIntValue {
        negative: sign == Sign::Minus,
        magnitude,
    }
}

/// Decodes sign + big-endian magnitude back to a `BigInt`.
///
/// An all-zero/empty magnitude decodes to zero regardless of the sign flag —
/// `BigInt` normalizes the sign of zero, so no invalid state is possible.
fn bigint_from_proto(bi: &pb::BigIntValue) -> BigInt {
    let sign = if bi.negative { Sign::Minus } else { Sign::Plus };
    BigInt::from_bytes_be(sign, &bi.magnitude)
}

fn value_list(items: &[MontyObject]) -> pb::ValueList {
    pb::ValueList {
        items: values_to_proto(items),
    }
}

fn dict_to_proto(pairs: &DictPairs) -> pb::DictValue {
    pb::DictValue {
        pairs: pairs
            .into_iter()
            .map(|(key, value)| pb::Pair {
                key: Some(key.into()),
                value: Some(value.into()),
            })
            .collect(),
    }
}

fn dict_from_proto(dict: pb::DictValue) -> Result<DictPairs, ProtoConvertError> {
    Ok(pairs_from_proto(dict.pairs)?.into())
}

/// Validates wire year/month/day fields against the invariants documented on
/// `MontyDate`/`MontyDateTime` (year 1..=9999, month 1..=12, day valid for the
/// month/year). The wire is untrusted, and an out-of-range date would corrupt
/// comparison, arithmetic, and formatting once inside the sandbox.
/// `fields` names the year/month/day wire fields for error messages.
fn date_fields(year: i32, month: u32, day: u32, fields: [&'static str; 3]) -> Result<(i32, u8, u8), ProtoConvertError> {
    let [year_field, month_field, day_field] = fields;
    if !(1..=9999).contains(&year) {
        return Err(ProtoConvertError::InvalidValue {
            field: year_field,
            reason: format!("{year} is outside the range 1..=9999"),
        });
    }
    let month = ranged_u8(month, 1..=12, month_field)?;
    let day = ranged_u8(day, 1..=u32::from(days_in_month(year, month)), day_field)?;
    Ok((year, month, day))
}

/// Days in a Gregorian month; `month` must already be validated to 1..=12.
fn days_in_month(year: i32, month: u8) -> u8 {
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Checks a wire `u32` against an inclusive range and narrows it to `u8`.
fn ranged_u8(value: u32, range: RangeInclusive<u32>, field: &'static str) -> Result<u8, ProtoConvertError> {
    if range.contains(&value) {
        Ok(u8::try_from(value).expect("range bounds fit in u8"))
    } else {
        Err(ProtoConvertError::InvalidValue {
            field,
            reason: format!("{value} is outside the range {}..={}", range.start(), range.end()),
        })
    }
}

/// Checks a wire `i32` against the half-open normalized range `0..max`.
fn normalized(value: i32, max: i32, field: &'static str) -> Result<i32, ProtoConvertError> {
    if (0..max).contains(&value) {
        Ok(value)
    } else {
        Err(ProtoConvertError::InvalidValue {
            field,
            reason: format!("{value} is outside the normalized range 0..{max}"),
        })
    }
}

/// Checks a wire `u32` against an inclusive upper bound.
fn bounded(value: u32, max: u32, field: &'static str) -> Result<u32, ProtoConvertError> {
    if value <= max {
        Ok(value)
    } else {
        Err(ProtoConvertError::InvalidValue {
            field,
            reason: format!("{value} exceeds maximum {max}"),
        })
    }
}
