//! `zoneinfo.ZoneInfo`: an IANA time zone resolved against the tz database the
//! worker carries, usable as a `datetime` `tzinfo`.
//!
//! The object holds its key and the resolved [`SandboxTimeZone`]; `datetime`
//! stores the offset and abbreviation in force at each value and keeps a
//! reference to the zone, re-resolving them when the wall clock moves (see
//! `TzInfoArg` in `datetime`). There is no instance cache, so two `ZoneInfo`
//! objects with one key are equal but not identical.

use std::{
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
};

use chrono::NaiveDateTime;
use monty_types::{MontyTimeZone, SandboxTimeZone};

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    hash::HashValue,
    heap::{DropGuard, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput},
    intern::StaticStrings,
    types::{
        CmpOrder, LazyHeapSet, PyTrait, Set, Type, datetime,
        str::{StringRepr, allocate_string},
        timezone,
    },
    value::{EitherStr, Value},
};

/// A resolved IANA zone.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ZoneInfo {
    /// The key as given, which the database spells the same way.
    key: String,
    /// The zone's rules; serializes as its name and re-resolves on load.
    zone: SandboxTimeZone,
}

impl ZoneInfo {
    /// The zone's transition rules.
    pub(crate) fn zone(&self) -> &SandboxTimeZone {
        &self.zone
    }

    /// The IANA key, for `repr()` and `str()`.
    pub(crate) fn key(&self) -> &str {
        &self.key
    }
}

impl HeapItem for ZoneInfo {
    fn py_dec_ref_ids(&mut self, _: &mut Vec<HeapId>) {}
}

/// `ZoneInfo(key)`, also `ZoneInfo.no_cache(key)`: resolves `key` against the tz
/// database, with CPython's key validation and its `ZoneInfoNotFoundError`
/// (a `KeyError` here).
pub(crate) fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let KeyArgs { key } = KeyArgs::from_args(args, vm)?;
    defer_drop!(key, vm);
    if key.is_str(vm.heap) {
        let key = key.to_str(vm)?.to_owned();
        check_key(&key)?;
        let zone = SandboxTimeZone::named(&key)
            .map_err(|_| SimpleException::new_msg(ExcType::KeyError, format!("No time zone found with key {key}")))?;
        Ok(Value::Ref(vm.heap.allocate(HeapData::ZoneInfo(ZoneInfo { key, zone }))))
    } else {
        Err(ExcType::type_error_fspath(&key.py_type_name(vm)))
    }
}

/// CPython's `ZoneInfo` key checks, in its order, each a `ValueError`.
fn check_key(key: &str) -> RunResult<()> {
    let problem = if key.starts_with('/') {
        Some("may not be absolute paths")
    } else if key.split('/').any(|part| part == "..") {
        Some("must refer to subdirectories of TZPATH")
    } else if key.split('/').any(|part| matches!(part, "" | ".")) {
        Some("must be normalized relative paths")
    } else {
        None
    };
    problem.map_or(Ok(()), |problem| {
        Err(ExcType::value_error(format!("ZoneInfo keys {problem}, got: {key}")))
    })
}

/// `ZoneInfo.clear_cache(*, only_keys=None)`: there is no instance cache, so
/// `only_keys` is accepted and released unread (CPython iterates it).
pub(crate) fn class_clear_cache(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let ClearCacheArgs { only_keys } = ClearCacheArgs::from_args(args, vm)?;
    only_keys.drop_with(vm);
    Ok(Value::None)
}

/// `zoneinfo.available_timezones()`: the keys of the tz database, as a set.
pub(crate) fn available_timezones(vm: &mut VM<'_>) -> RunResult<Value> {
    // Guarded so a resource error part-way releases the names already added.
    let mut names_guard = DropGuard::new(Set::new(), vm);
    let (names, vm) = names_guard.as_parts_mut();
    for name in SandboxTimeZone::available_names() {
        names.add(allocate_string(name, vm.heap), vm)?;
    }
    let (names, vm) = names_guard.into_parts();
    Ok(Value::Ref(vm.heap.allocate(HeapData::Set(names))))
}

/// Argument shape for `ZoneInfo(key)`: `at_most_total` gives CPython's
/// `ZoneInfo() takes at most 1 argument (2 given)`.
#[derive(FromArgs)]
#[from_args(name = "ZoneInfo", style = c_named, at_most_total)]
struct KeyArgs {
    key: Value,
}

/// Argument shape for `ZoneInfo.clear_cache(*, only_keys=None)`.
#[derive(FromArgs)]
#[from_args(name = "clear_cache", style = c_named)]
struct ClearCacheArgs {
    #[from_args(kw_only, default = Value::None)]
    only_keys: Value,
}

/// The single `dt` argument of `utcoffset`, `tzname` and `dst`, with the two
/// arity messages CPython's method wrapper writes.
fn dt_argument(method: StaticStrings, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let name: &'static str = method.into();
    let count = args.count();
    let positional = args.into_pos_only(name, vm.heap)?;
    defer_drop_mut!(positional, vm);
    let (first, second) = (positional.next(), positional.next());
    match (first, second) {
        (Some(dt), None) => Ok(dt),
        (first, second) => {
            // Both values were taken off the iterator, so the guard cannot release them.
            if let Some(value) = first {
                value.drop_with(vm);
            }
            if let Some(value) = second {
                value.drop_with(vm);
            }
            Err(ExcType::type_error(if count == 0 {
                format!("{name}() takes exactly 1 positional argument (0 given)")
            } else {
                format!("{name}() takes at most 1 argument ({count} given)")
            }))
        }
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, ZoneInfo> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::ZoneInfo
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(match other.read_heap(vm) {
            Some(HeapReadOutput::ZoneInfo(other)) => Some(self.get(vm.heap).key == other.get(vm.heap).key),
            _ => None,
        })
    }

    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        let mut hasher = DefaultHasher::new();
        self.get(vm.heap).key.hash(&mut hasher);
        Ok(Some(HashValue::new(hasher.finish())))
    }

    fn py_cmp(&self, _other: &Self, _vm: &mut VM<'h>) -> RunResult<CmpOrder> {
        Ok(CmpOrder::Incomparable)
    }

    fn py_bool(&self, _vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(true)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        write!(f, "zoneinfo.ZoneInfo(key={})", StringRepr(&self.get(vm.heap).key))?;
        Ok(())
    }

    fn py_str(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        Ok(allocate_string(self.get(vm.heap).key.clone(), vm.heap))
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        match attr.static_string(vm.interns) {
            Some(StaticStrings::Key) => Ok(Some(CallResult::Value(allocate_string(
                self.get(vm.heap).key.clone(),
                vm.heap,
            )))),
            _ => Ok(None),
        }
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let zone = self.get(vm.heap).zone.clone();
        match attr.static_string(vm.interns) {
            Some(method @ (StaticStrings::Utcoffset | StaticStrings::Tzname | StaticStrings::Dst)) => {
                let dt = dt_argument(method, args, vm)?;
                defer_drop!(dt, vm);
                match wall_clock_arg(dt, vm)? {
                    // `utcoffset(None)` and friends answer `None`, as CPython does.
                    None => Ok(Value::None),
                    Some(wall) => {
                        let offset = resolve_local(&zone, wall)?;
                        Ok(match method {
                            StaticStrings::Utcoffset => timezone::utcoffset_value(Some(offset.offset_seconds), vm.heap),
                            StaticStrings::Tzname => allocate_string(
                                timezone::tzname_string(offset.offset_seconds, offset.name.as_deref()),
                                vm.heap,
                            ),
                            _ => timezone::utcoffset_value(zone.dst_seconds_for_local(wall), vm.heap),
                        })
                    }
                }
            }
            Some(StaticStrings::Fromutc) => {
                let dt = args.get_one_arg("fromutc", vm.heap)?;
                defer_drop!(dt, vm);
                match dt.read_heap(vm) {
                    Some(HeapReadOutput::DateTime(datetime)) => {
                        let (utc, tzinfo_ref) = {
                            let datetime = datetime.get(vm.heap);
                            (datetime.naive(), datetime.tzinfo_ref())
                        };
                        if tzinfo_ref == Some(self.id()) {
                            datetime::allocate_in_zone(utc, &zone, self.id(), vm.heap)
                        } else {
                            Err(ExcType::value_error("fromutc: dt.tzinfo is not self"))
                        }
                    }
                    _ => Err(ExcType::type_error("fromutc: argument must be a datetime")),
                }
            }
            _ => Err(ExcType::attribute_error_method(Type::ZoneInfo, attr, args, vm)),
        }
        .map(CallResult::Value)
    }
}

/// The wall clock of a `dt` argument: `None` for `None`, the naive fields of a
/// `datetime`, and CPython's attribute errors for anything else.
fn wall_clock_arg(dt: &Value, vm: &VM<'_>) -> RunResult<Option<NaiveDateTime>> {
    match dt {
        Value::None => Ok(None),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::DateTime(datetime) => Ok(Some(datetime.naive())),
            // CPython reads the fields it needs straight off the argument.
            HeapData::Date(_) => Err(ExcType::attribute_error("datetime.date", "hour")),
            _ => Err(ExcType::attribute_error(dt.py_type_name(vm), "toordinal")),
        },
        _ => Err(ExcType::attribute_error(dt.py_type_name(vm), "toordinal")),
    }
}

/// The offset in force for a wall-clock time in `zone`: its first occurrence in a
/// fold, the pre-transition offset in a gap.
fn resolve_local(zone: &SandboxTimeZone, wall: NaiveDateTime) -> RunResult<MontyTimeZone> {
    zone.info_for_local(wall).ok_or_else(out_of_range)
}

fn out_of_range() -> RunError {
    SimpleException::new_msg(ExcType::OverflowError, "date value out of range").into()
}
