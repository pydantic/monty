//! Interpreter-side bridge for the boundary value types in `monty-types`:
//! conversions between [`MontyObject`]/[`MontyType`] and the VM's internal
//! `Value`/`Type` representations. The types themselves (and their pure
//! methods — reprs, hashing, truthiness) live in `monty-types`.

use ahash::AHashSet;
use monty_types::{
    DictPairs, InvalidInputError, MontyDate, MontyDateTime, MontyFileHandle, MontyObject, MontyTimeDelta,
    MontyTimeZone, MontyType, ResourceTracker,
};

use crate::{
    builtins::Builtins,
    bytecode::VM,
    defer_drop,
    exception_private::{RunError, SimpleException},
    heap::{DropGuard, Heap, HeapData, HeapId, HeapReadOutput},
    intern::Interns,
    types::{
        Dataclass, LongInt, NamedTuple, OpenFile, Path, PyTrait, TimeZone, Type, allocate_tuple,
        bytes::Bytes,
        date as date_type, datetime as datetime_type,
        dict::Dict,
        instance::class_name,
        list::List,
        set::{FrozenSet, Set},
        str::allocate_string,
        timedelta as timedelta_type,
    },
    value::{EitherStr, Value},
};

/// Crate-internal conversions between [`MontyObject`] and the VM's `Value`.
///
/// `MontyObject` lives in `monty-types`; building one from a heap `Value` (and
/// back) requires the VM, so the conversions stay here as a `pub(crate)`
/// extension trait.
pub(crate) trait MontyObjectExt: Sized {
    /// Converts a `Value` into a `MontyObject`, properly handling reference
    /// counting: takes ownership of the `Value` and drops it via `drop_with`.
    fn new(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> Self;

    /// Converts this `MontyObject` into a `Value`, allocating on the heap if
    /// needed. Fails with `InvalidInputError` on output-only variants
    /// (`Repr`, `Cycle`, sandbox class `Type`s) or when a resource limit is hit.
    fn to_value(self, vm: &mut VM<'_, impl ResourceTracker>) -> Result<Value, InvalidInputError>;

    /// Top-level entry into [`from_value_inner`](Self::from_value_inner),
    /// allocating the visited-set used for cycle detection.
    fn from_value(object: &Value, vm: &mut VM<'_, impl ResourceTracker>) -> Self;

    /// Converts a `Value` to a `MontyObject` with cycle detection via `visited`.
    fn from_value_inner(object: &Value, vm: &mut VM<'_, impl ResourceTracker>, visited: &mut AHashSet<HeapId>) -> Self;
}

impl MontyObjectExt for MontyObject {
    /// Converts a `Value` into a `MontyObject`, properly handling reference counting.
    ///
    /// Takes ownership of the `Value`, extracts its content to create a MontyObject,
    /// then properly drops the Value via `drop_with` to maintain reference counting.
    ///
    /// The `interns` parameter is used to look up interned string/bytes content.
    fn new(value: Value, vm: &mut VM<'_, impl ResourceTracker>) -> Self {
        let py_obj = Self::from_value(&value, vm);
        value.drop_with(vm);
        py_obj
    }

    /// Converts this `MontyObject` into an `Value`, allocating on the heap if needed.
    ///
    /// Immediate values (None, Bool, Int, Float, Ellipsis, Exception) are created directly.
    /// Heap-allocated values (String, Bytes, List, Tuple, Dict) are allocated
    /// via the heap and wrapped in `Value::Ref`.
    ///
    /// # Errors
    /// Returns `InvalidInputError` if called on the `Repr` variant,
    /// as it is only valid as an output from code execution, not as an input.
    fn to_value(self, vm: &mut VM<'_, impl ResourceTracker>) -> Result<Value, InvalidInputError> {
        match self {
            Self::Ellipsis => Ok(Value::Ellipsis),
            Self::None => Ok(Value::None),
            Self::Bool(b) => Ok(Value::Bool(b)),
            Self::Int(i) => Ok(Value::Int(i)),
            Self::BigInt(bi) => Ok(LongInt::new(bi).into_value(vm.heap)?),
            Self::Float(f) => Ok(Value::Float(f)),
            Self::String(s) => Ok(allocate_string(s, vm.heap)?),
            Self::Bytes(b) => Ok(Value::Ref(vm.heap.allocate(HeapData::Bytes(Bytes::new(b)))?)),
            Self::List(items) => {
                let values = convert_values(items, vm)?;
                Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(values)))?))
            }
            Self::Tuple(items) => {
                let values = convert_values(items, vm)?;
                allocate_tuple(values.into(), vm.heap).map_err(InvalidInputError::Resource)
            }
            Self::NamedTuple {
                type_name,
                field_names,
                values,
            } => {
                // `NamedTuple::new` asserts equal lengths; malformed host input
                // (e.g. untrusted serialized data) must error, not panic.
                if field_names.len() != values.len() {
                    return Err(InvalidInputError::invalid_type(
                        "NamedTuple field_names and values must have the same length",
                    ));
                }
                let values = convert_values(values, vm)?;
                let field_name_strs: Vec<EitherStr> = field_names.into_iter().map(Into::into).collect();
                let nt = NamedTuple::new(type_name, field_name_strs, values);
                Ok(Value::Ref(vm.heap.allocate(HeapData::NamedTuple(nt))?))
            }
            Self::Dict(map) => {
                let pairs = convert_pairs(map, vm)?;
                let dict =
                    Dict::from_pairs(pairs, vm).map_err(|_| InvalidInputError::invalid_type("unhashable dict keys"))?;
                Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))?))
            }
            Self::Set(items) => {
                let set = convert_set(items, vm, "unhashable set element")?;
                Ok(Value::Ref(vm.heap.allocate(HeapData::Set(set))?))
            }
            Self::FrozenSet(items) => {
                let set = convert_set(items, vm, "unhashable frozenset element")?;
                let frozenset = FrozenSet::from_set(set);
                Ok(Value::Ref(vm.heap.allocate(HeapData::FrozenSet(frozenset))?))
            }
            Self::Date(date) => {
                let value = date_type::from_ymd(date.year, i32::from(date.month), i32::from(date.day))
                    .map_err(|_| InvalidInputError::invalid_type("date"))?;
                Ok(Value::Ref(vm.heap.allocate(HeapData::Date(value))?))
            }
            Self::DateTime(datetime) => {
                let MontyDateTime {
                    year,
                    month,
                    day,
                    hour,
                    minute,
                    second,
                    microsecond,
                    offset_seconds,
                    timezone_name,
                } = datetime;
                if offset_seconds.is_none() && timezone_name.is_some() {
                    return Err(InvalidInputError::invalid_type("datetime"));
                }
                let tzinfo = offset_seconds
                    .map(|offset| TimeZone::new(offset, timezone_name))
                    .transpose()
                    .map_err(|_| InvalidInputError::invalid_type("datetime"))?;
                let value = datetime_type::from_components(
                    year,
                    i32::from(month),
                    i32::from(day),
                    i32::from(hour),
                    i32::from(minute),
                    i32::from(second),
                    i32::try_from(microsecond).map_err(|_| InvalidInputError::invalid_type("datetime"))?,
                    tzinfo,
                    None,
                    vm.heap,
                )
                .map_err(|_| InvalidInputError::invalid_type("datetime"))?;
                Ok(Value::Ref(vm.heap.allocate(HeapData::DateTime(value))?))
            }
            Self::TimeDelta(delta) => {
                let delta = timedelta_type::new(delta.days, delta.seconds, delta.microseconds)
                    .map_err(|_| InvalidInputError::invalid_type("timedelta"))?;
                Ok(Value::Ref(vm.heap.allocate(HeapData::TimeDelta(delta))?))
            }
            Self::TimeZone(tz) => {
                if tz.offset_seconds == 0 && tz.name.is_none() {
                    vm.heap
                        .get_timezone_utc()
                        .map_err(|_| InvalidInputError::invalid_type("timezone"))
                } else {
                    let tz = TimeZone::new(tz.offset_seconds, tz.name)
                        .map_err(|_| InvalidInputError::invalid_type("timezone"))?;
                    Ok(Value::Ref(vm.heap.allocate(HeapData::TimeZone(tz))?))
                }
            }
            Self::Exception { exc_type, arg } => {
                let exc = SimpleException::new(exc_type, arg);
                Ok(Value::Ref(vm.heap.allocate(HeapData::Exception(exc))?))
            }
            Self::Dataclass {
                name,
                type_id,
                field_names,
                attrs,
                frozen,
            } => {
                let pairs = convert_pairs(attrs, vm)?;
                let dict = Dict::from_pairs(pairs, vm)
                    .map_err(|_| InvalidInputError::invalid_type("unhashable dataclass attr keys"))?;
                let dc = Dataclass::new(name, type_id, field_names, dict, frozen);
                Ok(Value::Ref(vm.heap.allocate(HeapData::Dataclass(dc))?))
            }
            Self::Path(s) => Ok(Value::Ref(vm.heap.allocate(HeapData::Path(Path::new(s)))?)),
            Self::FileHandle(handle) => {
                let file = OpenFile::with_state(handle.path, handle.mode, handle.position);
                Ok(Value::Ref(vm.heap.allocate(HeapData::OpenFile(file))?))
            }
            Self::Type(t) => match t.to_internal() {
                Some(ty) => Ok(Value::Builtin(Builtins::Type(ty))),
                // `MontyType::Instance` carries only a class name — the class
                // binding cannot be reconstructed inside the sandbox (see the
                // invariant on the runtime `Type::Instance` variant).
                None => Err(InvalidInputError::invalid_type(
                    "a sandbox class type object is not a valid input value",
                )),
            },
            Self::BuiltinFunction(f) => Ok(Value::Builtin(Builtins::Function(f))),
            Self::Function { name, .. } => {
                // Try to intern the function name. If the name is already interned
                // (common case: the function has the same name as the variable it was
                // assigned to), use the lightweight `Value::ExtFunction(StringId)`.
                // Otherwise, allocate a `HeapData::ExtFunction(String)` on the heap.
                if let Some(string_id) = vm.interns.get_string_id_by_name(&name) {
                    Ok(Value::ExtFunction(string_id))
                } else {
                    Ok(Value::Ref(vm.heap.allocate(HeapData::ExtFunction(name))?))
                }
            }
            Self::Repr(_) => Err(InvalidInputError::invalid_type("'Repr' is not a valid input value")),
            Self::Cycle(_, _) => Err(InvalidInputError::invalid_type("'Cycle' is not a valid input value")),
        }
    }

    /// Top-level entry into [`from_value_inner`], allocating the visited-set used
    /// for cycle detection.
    fn from_value(object: &Value, vm: &mut VM<'_, impl ResourceTracker>) -> Self {
        let mut visited = AHashSet::new();
        Self::from_value_inner(object, vm, &mut visited)
    }

    /// Internal helper for converting Value to MontyObject with cycle detection.
    ///
    /// Non-`Ref` variants are produced inline using only the interner — they
    /// never recurse through the heap. `Ref` variants dispatch via
    /// `vm.heap.read(id)` so the resulting [`HeapRead`] keeps the heap entry
    /// alive (through its reader count) without retaining a borrow on
    /// `vm.heap`. Recursing can run a user-defined `__repr__` (via
    /// [`repr_or_error`] on nested instances), so mutable containers (list,
    /// dict, set, dataclass attrs) snapshot ALL children up front — the
    /// `inc_ref`s keep each child alive and the snapshot keeps iteration valid
    /// even if that `__repr__` mutates the container. Immutable containers
    /// (tuple, namedtuple, frozenset) clone per-item: their length and slots
    /// cannot change mid-iteration.
    fn from_value_inner(object: &Value, vm: &mut VM<'_, impl ResourceTracker>, visited: &mut AHashSet<HeapId>) -> Self {
        // Check depth limit before processing
        let Ok(mut guard) = vm.recursion_guard() else {
            return Self::Repr("<deeply nested>".to_owned());
        };
        let vm = &mut *guard;

        let interns = vm.interns;
        match object {
            Value::Undefined => panic!("Undefined found while converting to MontyObject"),
            Value::Ellipsis => Self::Ellipsis,
            Value::None => Self::None,
            Value::Bool(b) => Self::Bool(*b),
            Value::Int(i) => Self::Int(*i),
            Value::Float(f) => Self::Float(*f),
            Value::InternString(string_id) => Self::String(interns.get_str(*string_id).to_owned()),
            Value::InternBytes(bytes_id) => Self::Bytes(interns.get_bytes(*bytes_id).to_owned()),
            Value::InternLongInt(li_id) => Self::BigInt(interns.get_long_int(*li_id).clone()),
            Value::Ref(id) => {
                // Check for cycle
                if visited.contains(id) {
                    // Cycle detected - return appropriate placeholder
                    return match vm.heap.get(*id) {
                        HeapData::List(_) => Self::Cycle(id.index(), "[...]".to_owned()),
                        HeapData::Tuple(_) | HeapData::NamedTuple(_) => Self::Cycle(id.index(), "(...)".to_owned()),
                        HeapData::Dict(_) => Self::Cycle(id.index(), "{...}".to_owned()),
                        _ => Self::Cycle(id.index(), "...".to_owned()),
                    };
                }

                // Mark this id as being visited
                visited.insert(*id);

                let result = match vm.heap.read(*id) {
                    HeapReadOutput::Str(s) => Self::String(s.get(vm.heap).as_str().to_owned()),
                    HeapReadOutput::Bytes(b) => Self::Bytes(b.get(vm.heap).as_slice().to_owned()),
                    HeapReadOutput::List(list) => {
                        // Snapshot before recursing: a nested `__repr__` may mutate this list.
                        let children: Vec<Value> = list
                            .get(vm.heap)
                            .as_slice()
                            .iter()
                            .map(|item| item.clone_with_heap(vm.heap))
                            .collect();
                        defer_drop!(children, vm);
                        Self::List(values_to_objects(children, vm, visited))
                    }
                    HeapReadOutput::Tuple(tuple) => {
                        let len = tuple.get(vm.heap).as_slice().len();
                        let mut items = Vec::with_capacity(len);
                        for i in 0..len {
                            let item = tuple.get(vm.heap).as_slice()[i].clone_with_heap(vm.heap);
                            defer_drop!(item, vm);
                            items.push(Self::from_value_inner(item, vm, visited));
                        }
                        Self::Tuple(items)
                    }
                    HeapReadOutput::NamedTuple(nt) => {
                        let type_name = nt.get(vm.heap).name(vm.interns).to_owned();
                        let field_names = nt
                            .get(vm.heap)
                            .field_names()
                            .iter()
                            .map(|fname| fname.as_str(vm.interns).to_owned())
                            .collect::<Vec<_>>();
                        let len = nt.get(vm.heap).len();
                        let mut values = Vec::with_capacity(len);
                        for i in 0..len {
                            let item = nt.get(vm.heap).as_vec()[i].clone_with_heap(vm.heap);
                            defer_drop!(item, vm);
                            values.push(Self::from_value_inner(item, vm, visited));
                        }
                        Self::NamedTuple {
                            type_name,
                            field_names,
                            values,
                        }
                    }
                    HeapReadOutput::Dict(dict) => {
                        // Snapshot before recursing: a nested `__repr__` may mutate this dict.
                        let children = snapshot_dict_pairs(dict.get(vm.heap), vm.heap);
                        defer_drop!(children, vm);
                        Self::Dict(pairs_to_objects(children, vm, visited).into())
                    }
                    HeapReadOutput::Set(set) => {
                        // Snapshot before recursing: a nested `__repr__` may mutate this set.
                        let children: Vec<Value> = {
                            let set_ref = set.get(vm.heap);
                            (0..set_ref.len())
                                .map(|i| {
                                    set_ref
                                        .storage()
                                        .value_at(i)
                                        .expect("index in range")
                                        .clone_with_heap(vm.heap)
                                })
                                .collect()
                        };
                        defer_drop!(children, vm);
                        Self::Set(values_to_objects(children, vm, visited))
                    }
                    HeapReadOutput::FrozenSet(fs) => {
                        let len = fs.get(vm.heap).len();
                        let mut items = Vec::with_capacity(len);
                        for i in 0..len {
                            let item = fs
                                .get(vm.heap)
                                .storage()
                                .value_at(i)
                                .expect("index in range")
                                .clone_with_heap(vm.heap);
                            defer_drop!(item, vm);
                            items.push(Self::from_value_inner(item, vm, visited));
                        }
                        Self::FrozenSet(items)
                    }
                    // Cells are internal closure implementation details — show
                    // the contents directly without exposing the wrapper.
                    HeapReadOutput::Cell(cell) => {
                        let inner = cell.get(vm.heap).0.clone_with_heap(vm.heap);
                        defer_drop!(inner, vm);
                        Self::from_value_inner(inner, vm, visited)
                    }
                    HeapReadOutput::Date(d) => {
                        let (year, month, day) = date_type::to_ymd(*d.get(vm.heap));
                        Self::Date(MontyDate {
                            year,
                            month: u8::try_from(month).expect("month is always 1..=12"),
                            day: u8::try_from(day).expect("day is always 1..=31"),
                        })
                    }
                    HeapReadOutput::DateTime(dt) => {
                        if let Some((year, month, day, hour, minute, second, microsecond)) =
                            datetime_type::to_components(dt.get(vm.heap))
                        {
                            Self::DateTime(MontyDateTime {
                                year,
                                month,
                                day,
                                hour,
                                minute,
                                second,
                                microsecond,
                                offset_seconds: datetime_type::offset_seconds(dt.get(vm.heap)),
                                timezone_name: datetime_type::timezone_info(dt.get(vm.heap)).and_then(|tz| tz.name),
                            })
                        } else {
                            repr_or_error(object, vm)
                        }
                    }
                    HeapReadOutput::TimeDelta(td) => {
                        let (days, seconds, microseconds) = timedelta_type::components(td.get(vm.heap));
                        Self::TimeDelta(MontyTimeDelta {
                            days,
                            seconds,
                            microseconds,
                        })
                    }
                    HeapReadOutput::TimeZone(tz) => {
                        let tz_ref = tz.get(vm.heap);
                        Self::TimeZone(MontyTimeZone {
                            offset_seconds: tz_ref.offset_seconds,
                            name: tz_ref.name.clone(),
                        })
                    }
                    HeapReadOutput::Exception(exc) => {
                        let exc_ref = exc.get(vm.heap);
                        Self::Exception {
                            exc_type: exc_ref.exc_type(),
                            arg: exc_ref.arg().map(ToString::to_string),
                        }
                    }
                    HeapReadOutput::Dataclass(dc) => {
                        let (name, type_id, field_names, frozen) = {
                            let dc_ref = dc.get(vm.heap);
                            (
                                dc_ref.name(vm.interns).to_owned(),
                                dc_ref.type_id(),
                                dc_ref.field_names().to_vec(),
                                dc_ref.is_frozen(),
                            )
                        };
                        // Snapshot before recursing: attrs are mutable via `setattr`.
                        let children = snapshot_dict_pairs(dc.get(vm.heap).attrs(), vm.heap);
                        defer_drop!(children, vm);
                        Self::Dataclass {
                            name,
                            type_id,
                            field_names,
                            attrs: pairs_to_objects(children, vm, visited).into(),
                            frozen,
                        }
                    }
                    // Iterators are internal objects — represent as a fixed type
                    // string rather than recursing.
                    HeapReadOutput::Iter(_) => Self::Repr("<iterator>".to_owned()),
                    HeapReadOutput::LongInt(li) => Self::BigInt(li.get(vm.heap).inner().clone()),
                    HeapReadOutput::Module(m) => {
                        Self::Repr(format!("<module '{}'>", vm.interns.get_str(m.get(vm.heap).name())))
                    }
                    HeapReadOutput::Coroutine(coro) => {
                        let func_id = coro.get(vm.heap).func_id;
                        let func = vm.interns.get_function(func_id);
                        let name = vm.interns.get_str(func.name.name_id);
                        Self::Repr(format!("<coroutine object {name}>"))
                    }
                    HeapReadOutput::GatherFuture(gather) => {
                        Self::Repr(format!("<gather({})>", gather.get(vm.heap).item_count()))
                    }
                    HeapReadOutput::Path(path) => Self::Path(path.get(vm.heap).as_str().to_owned()),
                    // File objects carry no heap refs (leaf type) — no recursion.
                    // This is how `file.read()`/`write()` deliver the open file
                    // to the host as the first OS-call argument.
                    HeapReadOutput::OpenFile(file) => {
                        let file = file.get(vm.heap);
                        Self::FileHandle(MontyFileHandle {
                            path: file.path().to_owned(),
                            mode: *file.file_mode(),
                            position: file.position(),
                        })
                    }
                    HeapReadOutput::ExtFunction(name) => Self::Function {
                        name: name.get(vm.heap).clone(),
                        docstring: None,
                    },
                    _ => repr_or_error(object, vm),
                };

                // Remove from visited set after processing
                visited.remove(id);
                result
            }
            Value::Builtin(Builtins::Type(t)) => Self::Type(MontyType::from_internal(*t, vm.heap, vm.interns)),
            Value::Builtin(Builtins::ExcType(e)) => Self::Type(MontyType::Exception(*e)),
            Value::Builtin(Builtins::Function(f)) => Self::BuiltinFunction(*f),
            // Inline external function: export under the same shape as the heap
            // path's `HeapReadOutput::ExtFunction` arm above, so an interned
            // function name round-trips through Monty as `MontyObject::Function`
            // regardless of which representation it took (issue #345).
            Value::ExtFunction(name_id) => Self::Function {
                name: vm.interns.get_str(*name_id).to_owned(),
                docstring: None,
            },
            #[cfg(feature = "memory-model-checks")]
            Value::Dereferenced => panic!("Dereferenced found while converting to MontyObject"),
            _ => repr_or_error(object, vm),
        }
    }
}

/// Crate-internal bridge between [`MontyType`] and the runtime [`Type`].
///
/// `MontyType` lives in `monty-types` (it is pure data), but mapping it to and
/// from the runtime `Type` needs heap/intern access, so the conversions stay
/// here as a `pub(crate)` extension trait.
pub(crate) trait MontyTypeExt: Sized {
    fn to_internal(&self) -> Option<Type>;

    fn from_internal_static(ty: Type) -> Self;

    fn from_internal(ty: Type, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> Self;
}

impl MontyTypeExt for MontyType {
    /// The internal runtime [`Type`] this variant mirrors, or `None` for
    /// [`Instance`](Self::Instance) — a class binding cannot be reconstructed
    /// from a name, which is exactly why `Instance` inputs are rejected.
    ///
    /// Keep in lockstep with [`from_internal_static`](Self::from_internal_static);
    /// both matches are exhaustive so the compiler enforces totality.
    fn to_internal(&self) -> Option<Type> {
        match self {
            Self::Ellipsis => Some(Type::Ellipsis),
            Self::Type => Some(Type::Type),
            Self::NoneType => Some(Type::NoneType),
            Self::Bool => Some(Type::Bool),
            Self::Int => Some(Type::Int),
            Self::Float => Some(Type::Float),
            Self::Range => Some(Type::Range),
            Self::Slice => Some(Type::Slice),
            Self::Date => Some(Type::Date),
            Self::DateTime => Some(Type::DateTime),
            Self::TimeDelta => Some(Type::TimeDelta),
            Self::TimeZone => Some(Type::TimeZone),
            Self::Str => Some(Type::Str),
            Self::Bytes => Some(Type::Bytes),
            Self::List => Some(Type::List),
            Self::ListIterator => Some(Type::ListIterator),
            Self::CallableIterator => Some(Type::CallableIterator),
            Self::Tuple => Some(Type::Tuple),
            Self::NamedTuple => Some(Type::NamedTuple),
            Self::Dict => Some(Type::Dict),
            Self::DictKeys => Some(Type::DictKeys),
            Self::DictItems => Some(Type::DictItems),
            Self::DictValues => Some(Type::DictValues),
            Self::Set => Some(Type::Set),
            Self::FrozenSet => Some(Type::FrozenSet),
            Self::Dataclass => Some(Type::Dataclass),
            Self::Instance(_) => None,
            Self::Exception(exc_type) => Some(Type::Exception(*exc_type)),
            Self::Function => Some(Type::Function),
            Self::BuiltinFunction => Some(Type::BuiltinFunction),
            Self::Cell => Some(Type::Cell),
            Self::Iterator => Some(Type::Iterator),
            Self::Coroutine => Some(Type::Coroutine),
            Self::Module => Some(Type::Module),
            Self::TextIOWrapper => Some(Type::TextIOWrapper),
            Self::BufferedReader => Some(Type::BufferedReader),
            Self::BufferedWriter => Some(Type::BufferedWriter),
            Self::BufferedRandom => Some(Type::BufferedRandom),
            Self::SpecialForm => Some(Type::SpecialForm),
            Self::Path => Some(Type::Path),
            Self::Property => Some(Type::Property),
            Self::RePattern => Some(Type::RePattern),
            Self::ReMatch => Some(Type::ReMatch),
        }
    }

    /// Mirrors a runtime [`Type`] whose class identity is NOT needed. Use
    /// [`from_internal`](Self::from_internal) when a heap is available.
    ///
    /// # Panics
    /// On `Instance`, whose class name cannot be resolved without a heap; it
    /// is unreachable on every current call path (`Builtins::Type` and
    /// `from_type_name` never hold/produce it).
    fn from_internal_static(ty: Type) -> Self {
        match ty {
            Type::Ellipsis => Self::Ellipsis,
            Type::Type => Self::Type,
            Type::NoneType => Self::NoneType,
            Type::Bool => Self::Bool,
            Type::Int => Self::Int,
            Type::Float => Self::Float,
            Type::Range => Self::Range,
            Type::Slice => Self::Slice,
            Type::Date => Self::Date,
            Type::DateTime => Self::DateTime,
            Type::TimeDelta => Self::TimeDelta,
            Type::TimeZone => Self::TimeZone,
            Type::Str => Self::Str,
            Type::Bytes => Self::Bytes,
            Type::List => Self::List,
            Type::ListIterator => Self::ListIterator,
            Type::CallableIterator => Self::CallableIterator,
            Type::Tuple => Self::Tuple,
            Type::NamedTuple => Self::NamedTuple,
            Type::Dict => Self::Dict,
            Type::DictKeys => Self::DictKeys,
            Type::DictItems => Self::DictItems,
            Type::DictValues => Self::DictValues,
            Type::Set => Self::Set,
            Type::FrozenSet => Self::FrozenSet,
            Type::Dataclass => Self::Dataclass,
            Type::Instance(_) => unreachable!("Type::Instance requires heap access — use MontyType::from_internal"),
            Type::Exception(exc_type) => Self::Exception(exc_type),
            Type::Function => Self::Function,
            Type::BuiltinFunction => Self::BuiltinFunction,
            Type::Cell => Self::Cell,
            Type::Iterator => Self::Iterator,
            Type::Coroutine => Self::Coroutine,
            Type::Module => Self::Module,
            Type::TextIOWrapper => Self::TextIOWrapper,
            Type::BufferedReader => Self::BufferedReader,
            Type::BufferedWriter => Self::BufferedWriter,
            Type::BufferedRandom => Self::BufferedRandom,
            Type::SpecialForm => Self::SpecialForm,
            Type::Path => Self::Path,
            Type::Property => Self::Property,
            Type::RePattern => Self::RePattern,
            Type::ReMatch => Self::ReMatch,
        }
    }

    /// The total mirror of a runtime [`Type`]: `Instance` resolves its class
    /// name via the heap.
    fn from_internal(ty: Type, heap: &Heap<impl ResourceTracker>, interns: &Interns) -> Self {
        match ty {
            Type::Instance(class_id) => Self::Instance(class_name(class_id, heap, interns).into_owned()),
            other => Self::from_internal_static(other),
        }
    }
}

/// Converts a sequence of `MontyObject`s into runtime `Value`s, releasing all
/// already-converted values if a later conversion fails (invalid nested input
/// or a resource limit) so no refcounts leak on the error path.
fn convert_values(
    items: Vec<MontyObject>,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> Result<Vec<Value>, InvalidInputError> {
    let mut guard = DropGuard::new(Vec::with_capacity(items.len()), vm);
    let (values, vm) = guard.as_parts_mut();
    for item in items {
        values.push(item.to_value(vm)?);
    }
    Ok(guard.into_inner())
}

/// Converts `(key, value)` `MontyObject` pairs into runtime `Value` pairs with
/// the same all-paths cleanup guarantee as [`convert_values`].
fn convert_pairs(
    map: DictPairs,
    vm: &mut VM<'_, impl ResourceTracker>,
) -> Result<Vec<(Value, Value)>, InvalidInputError> {
    let mut guard = DropGuard::new(Vec::with_capacity(map.len()), vm);
    let (pairs, vm) = guard.as_parts_mut();
    for (key_obj, value_obj) in map {
        let key = key_obj.to_value(vm)?;
        // Guard the key while the value converts so a failing value doesn't leak it.
        let mut key_guard = DropGuard::new(key, &mut *vm);
        let value = value_obj.to_value(key_guard.ctx())?;
        pairs.push((key_guard.into_inner(), value));
    }
    Ok(guard.into_inner())
}

/// Builds a `Set` from `MontyObject` elements, dropping the partially-built
/// set (and every value already added to it) if any element fails to convert
/// or hash.
fn convert_set(
    items: Vec<MontyObject>,
    vm: &mut VM<'_, impl ResourceTracker>,
    unhashable_msg: &'static str,
) -> Result<Set, InvalidInputError> {
    let mut guard = DropGuard::new(Set::new(), vm);
    let (set, vm) = guard.as_parts_mut();
    for item in items {
        let value = item.to_value(vm)?;
        set.add(value, vm)
            .map_err(|_| InvalidInputError::invalid_type(unhashable_msg))?;
    }
    Ok(guard.into_inner())
}

/// Converts a guarded snapshot of container children to `MontyObject`s.
///
/// Taking a `&[Value]` snapshot (cloned and guarded by the caller) is what
/// keeps [`from_value_inner`](MontyObjectExt::from_value_inner) safe against a
/// nested `__repr__` mutating the source container mid-iteration.
fn values_to_objects(
    children: &[Value],
    vm: &mut VM<'_, impl ResourceTracker>,
    visited: &mut AHashSet<HeapId>,
) -> Vec<MontyObject> {
    children
        .iter()
        .map(|child| MontyObject::from_value_inner(child, vm, visited))
        .collect()
}

/// Converts a guarded snapshot of dict entries to `MontyObject` pairs — the
/// pair-wise counterpart of [`values_to_objects`].
fn pairs_to_objects(
    children: &[(Value, Value)],
    vm: &mut VM<'_, impl ResourceTracker>,
    visited: &mut AHashSet<HeapId>,
) -> Vec<(MontyObject, MontyObject)> {
    children
        .iter()
        .map(|(key, value)| {
            (
                MontyObject::from_value_inner(key, vm, visited),
                MontyObject::from_value_inner(value, vm, visited),
            )
        })
        .collect()
}

/// Clones every `(key, value)` pair out of a dict (or dataclass attrs) so
/// recursive conversion cannot be invalidated by user code mutating it.
fn snapshot_dict_pairs(dict: &Dict, heap: &Heap<impl ResourceTracker>) -> Vec<(Value, Value)> {
    (0..dict.len())
        .map(|i| {
            (
                dict.key_at(i).expect("index in range").clone_with_heap(heap),
                dict.value_at(i).expect("index in range").clone_with_heap(heap),
            )
        })
        .collect()
}

/// Converts a value to its repr string for `MontyObject`, falling back to a
/// descriptive error message if `py_repr` fails (e.g. INT_MAX_STR_DIGITS).
fn repr_or_error(value: &Value, vm: &mut VM<'_, impl ResourceTracker>) -> MontyObject {
    match value.py_repr(vm) {
        Ok(s) => {
            // `py_repr` yields a heap `str` `Value`; extract its text and drop it.
            defer_drop!(s, vm);
            MontyObject::Repr(s.to_str(vm).map(str::to_owned).unwrap_or_default())
        }
        Err(e) => {
            let ty = value.py_type_name(vm);
            let msg = match &e {
                RunError::Internal(s) => s.to_string(),
                RunError::Exc(exc) | RunError::UncatchableExc(exc) => exc.exc.to_string(),
            };
            MontyObject::Repr(format!("<{ty} object, error on repr(): {msg}>"))
        }
    }
}
