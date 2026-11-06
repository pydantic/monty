//! Mutable byte storage and scoped access to its contiguous buffer.

use std::{fmt::Write, iter::successors};

use crate::{
    args::{ArgValues, FromArgs, StrArg},
    bytecode::{CallResult, VM},
    codecs::Codec,
    defer_drop, defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    heap::{DropGuard, DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead, heap_read_ref_as_field},
    intern::StaticStrings,
    percent_format::{copy_bytes_template, percent_format_bytes},
    py_value::PyValue,
    resource_checks::check_repeat_size,
    types::{BytesIterator, CmpOrder, LazyHeapSet, PyTrait, Type, iter::checked_preallocation_hint},
    value::{EitherStr, VALUE_SIZE, Value, eq_bytes},
};

/// A bytearray owns its buffer; heap capabilities keep scoped borrows valid across aliases.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub(crate) struct ByteArray(#[serde(with = "serde_bytes")] Vec<u8>);

impl ByteArray {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// A mutable slice permits writes but cannot resize the buffer during a borrow.
    pub(crate) fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.0
    }

    pub(crate) fn allocate(bytes: Vec<u8>, vm: &VM<'_>) -> Value {
        Value::Ref(vm.heap.allocate(HeapData::ByteArray(Self::new(bytes))))
    }

    /// Constructs from encoded text, a zero-filled size, a buffer, or an iterable of byte values.
    pub(crate) fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let InitArgs {
            source,
            encoding,
            errors,
        } = InitArgs::from_args(args, vm)?;
        defer_drop!(source, vm);
        defer_drop!(encoding, vm);
        defer_drop!(errors, vm);

        // Encoding requires text; validate this before trying the index or iterator protocols.
        let source_str = source.as_ref().and_then(|value| value.as_str(vm));
        if let Some(encoding) = encoding {
            let Some(text) = source_str else {
                return Err(ExcType::type_error_encoding_without_string());
            };
            let encoding = encoding.as_str(vm);
            let codec = Codec::find(encoding).ok_or_else(|| ExcType::lookup_error_unknown_encoding(encoding))?;
            let errors = errors.as_ref().map_or("strict", |value| value.as_str(vm));
            return Ok(Self::allocate(codec.encode(text, errors, &vm.heap.tracker)?, vm));
        }

        if source_str.is_some() {
            return Err(ExcType::type_error_string_without_encoding());
        }
        if errors.is_some() {
            return Err(ExcType::type_error_errors_without_string());
        }

        // Prefer `__index__` over iteration when the source implements both protocols.
        let count = match source {
            Some(value) => value.py_index_impl(vm)?,
            None => None,
        };
        defer_drop!(count, vm);

        let data = match (source, count) {
            (_, Some(value)) => {
                let count = value.as_int(vm)?;
                if count < 0 {
                    return Err(ExcType::value_error_negative_bytes_count());
                }

                // Preflight the full buffer so a large size fails before reaching the allocator's hard limit.
                let count = usize::try_from(count).map_err(|_| ExcType::overflow_index_sized_int())?;
                check_repeat_size(count, 1, &vm.heap.tracker)?;
                let mut data = Vec::new();
                data.try_reserve_exact(count)
                    .map_err(|_| ExcType::allocation_too_large())?;

                data.resize(count, 0);
                data
            }
            (None, None) => Vec::new(),
            // Buffers are copied; other inputs must yield integers in `0..=255`.
            (Some(value), None) => collect_bytes(value, vm)?,
        };

        Ok(Self::allocate(data, vm))
    }
}

#[derive(FromArgs)]
#[from_args(name = "bytearray", at_most_total, bad_arg_named)]
struct InitArgs {
    #[from_args(default)]
    source: Option<Value>,
    #[from_args(default)]
    encoding: Option<StrArg>,
    #[from_args(default)]
    errors: Option<StrArg>,
}

/// Copies buffer providers before mutation, including self-assignment through aliases.
fn collect_bytes(value: &Value, vm: &mut VM<'_>) -> RunResult<Vec<u8>> {
    if let Some(bytes) = read_buffer(value, vm) {
        check_repeat_size(bytes.len(), 1, &vm.heap.tracker)?;
        return Ok(bytes.to_vec());
    }
    let iterator = value.py_iter(vm)?.require_synchronous(vm)?;
    defer_drop!(iterator, vm);
    let mut iterator = iterator.read(vm);
    let hint = checked_preallocation_hint(iterator.iter_size_hint(vm), 1, &vm.heap.tracker)?;
    let mut bytes = Vec::with_capacity(hint);
    while let Some(value) = iterator.py_next(vm)? {
        defer_drop!(value, vm);
        let byte = byte_value(value, vm)?;
        vm.heap.tracker.check_growth(bytes.len(), bytes.capacity(), 1)?;
        bytes.push(byte);
        vm.heap.tracker.check_time_every(bytes.len())?;
    }
    Ok(bytes)
}

fn byte_value(value: &Value, vm: &mut VM<'_>) -> RunResult<u8> {
    let n = value.saturating_index(vm)?;
    u8::try_from(n).map_err(|_| SimpleException::new_msg(ExcType::ValueError, "byte must be in range(0, 256)").into())
}

/// A buffer borrow is scoped to the VM borrow, preventing resize or release while in use.
pub(crate) fn read_buffer<'a>(value: &Value, vm: &'a VM<'_>) -> Option<&'a [u8]> {
    match value {
        Value::InternBytes(id) => Some(vm.interns.get_bytes(*id)),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Bytes(bytes) => Some(bytes.as_slice()),
            HeapData::ByteArray(bytes) => Some(bytes.as_slice()),
            _ => None,
        },
        _ => None,
    }
}

impl HeapItem for ByteArray {
    fn py_dec_ref_ids(&mut self, _: &mut Vec<HeapId>) {}
}

impl<'h> HeapObjectRead<'h, ByteArray> {
    pub(crate) fn allocate_copy(&self, vm: &VM<'h>) -> RunResult<Value> {
        check_repeat_size(self.get(vm.heap).0.len(), 1, &vm.heap.tracker)?;
        Ok(ByteArray::allocate(self.get(vm.heap).0.clone(), vm))
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, ByteArray> {
    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::ByteArray
    }

    fn py_len(&self, vm: &VM<'h>) -> Option<usize> {
        Some(self.get(vm.heap).0.len())
    }

    fn py_is_iterable(&self, _: &VM<'h>) -> bool {
        true
    }

    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        Ok(CallResult::Value(BytesIterator::from_heap(self.id(), vm)))
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(eq_bytes(self.get(vm.heap).as_slice(), other, vm))
    }

    fn py_cmp(&self, other: &Self, vm: &mut VM<'h>) -> RunResult<CmpOrder> {
        Ok(CmpOrder::Ordered(
            self.get(vm.heap).as_slice().cmp(other.get(vm.heap).as_slice()),
        ))
    }

    fn py_mod_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let template = copy_bytes_template(self.get(vm.heap).as_slice(), &vm.heap.tracker)?;
        let value = percent_format_bytes(&template, other, vm)?;
        convert_result(value, vm).map(Some)
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _: &mut LazyHeapSet) -> RunResult<()> {
        f.write_str("bytearray(")?;
        super::bytes::bytes_repr_fmt(self.get(vm.heap).as_slice(), f)?;
        Ok(f.write_char(')')?)
    }

    fn py_contains_impl(&self, item: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        super::bytes::bytes_contains(self.get(vm.heap).as_slice(), item, vm).map(Some)
    }

    fn py_getitem(&self, key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        if matches!(key, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Slice(_))) {
            let field = heap_read_ref_as_field!(self, ByteArray, 0);
            let value = super::bytes::bytes_getitem(&field.as_slice(), key, vm)?;
            convert_result(value, vm)
        } else {
            let index = key.as_index(vm, Type::ByteArray)?;
            let byte = super::bytes::get_byte_at_index(self.get(vm.heap).as_slice(), index)
                .ok_or_else(|| SimpleException::new_msg(ExcType::IndexError, "bytearray index out of range"))?;
            Ok(Value::Int(i64::from(byte)))
        }
    }

    /// Assigns one byte by index, or replaces a slice from a buffer or iterable of byte values.
    fn py_setitem(&mut self, key: Value, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        defer_drop!(key, vm);
        defer_drop!(value, vm);

        let slice = match key {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::Slice(slice) => Some(slice.clone()),
                _ => None,
            },
            _ => None,
        };

        if let Some(slice) = slice {
            if matches!(value.py_value(vm), PyValue::Int(_) | PyValue::Bool(_) | PyValue::Str(_)) {
                return Err(ExcType::type_error(
                    "can assign only bytes, buffers, or iterables of ints in range(0, 256)",
                ));
            }

            // Collect before writing replacement bytes, handling self-assignment and iterator errors.
            let replacement = collect_bytes(value, vm)?;

            // User iteration can mutate the destination, so normalize against its final length.
            let (start, stop, step) = slice.indices(self.get(vm.heap).0.len())?;

            if step == 1 {
                // Contiguous slices may resize; a reversed start/stop denotes an insertion point.
                let start = usize::try_from(start).expect("normalized slice start");
                let stop = (usize::try_from(stop).expect("normalized slice stop")).max(start);
                let new_len = self.get(vm.heap).0.len() - (stop - start) + replacement.len();
                check_repeat_size(new_len, 1, &vm.heap.tracker)?;
                self.get_mut(vm.heap).0.splice(start..stop, replacement);
            } else {
                // Extended slices cannot resize: validate the replacement length before writing any bytes.
                let indices = successors(Some(start), |i| i.checked_add(step))
                    .take_while(|i| if step > 0 { *i < stop } else { *i > stop })
                    .map(|i| usize::try_from(i).expect("normalized slice index"));
                let count = indices.clone().count();
                if count != replacement.len() {
                    return Err(SimpleException::new_msg(
                        ExcType::ValueError,
                        format!(
                            "attempt to assign bytes of size {} to extended slice of size {}",
                            replacement.len(),
                            count
                        ),
                    )
                    .into());
                }

                let data = self.get_mut(vm.heap);
                for (index, byte) in indices.zip(replacement) {
                    data.as_mut_slice()[index] = byte;
                }
            }
            return Ok(());
        }

        // Both conversions may call `__index__` and mutate this buffer; read its length afterward.
        let index = key.as_index(vm, Type::ByteArray)?;
        let byte = byte_value(value, vm)?;

        let len = i64::try_from(self.get(vm.heap).0.len()).expect("buffer length fits i64");
        let index = if index < 0 { index + len } else { index };
        let Some(slot) = usize::try_from(index)
            .ok()
            .and_then(|i| self.get_mut(vm.heap).0.get_mut(i))
        else {
            return Err(SimpleException::new_msg(ExcType::IndexError, "bytearray index out of range").into());
        };

        *slot = byte;
        Ok(())
    }

    fn py_iadd_impl(&mut self, other: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        if matches!(other, Value::Ref(id) if *id == self.id()) && !self.get(vm.heap).0.is_empty() {
            return Err(SimpleException::new_msg(
                ExcType::BufferError,
                "Existing exports of data: object cannot be re-sized",
            )
            .into());
        }
        let Some(bytes) = read_buffer(other, vm) else {
            return Ok(false);
        };
        let bytes = bytes.to_vec();
        check_repeat_size(
            self.get(vm.heap).0.len().saturating_add(bytes.len()),
            1,
            &vm.heap.tracker,
        )?;
        self.get_mut(vm.heap).0.extend(bytes);
        Ok(true)
    }

    fn py_imul_impl(&mut self, other: &Value, vm: &mut VM<'h>) -> RunResult<bool> {
        let Some(count) = other.py_value(vm).repeat_count(vm)? else {
            return Ok(false);
        };
        check_repeat_size(self.get(vm.heap).0.len(), count, &vm.heap.tracker)?;
        repeat_in_place(&mut self.get_mut(vm.heap).0, count)?;
        Ok(true)
    }

    fn py_mul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(count) = other.py_value(vm).repeat_count(vm)? else {
            return Ok(None);
        };
        check_repeat_size(self.get(vm.heap).0.len(), count, &vm.heap.tracker)?;
        let mut data = if count == 0 {
            Vec::new()
        } else {
            self.get(vm.heap).0.clone()
        };
        repeat_in_place(&mut data, count)?;
        Ok(Some(ByteArray::allocate(data, vm)))
    }

    fn py_rmul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        self.py_mul_impl(other, vm)
    }

    fn py_add_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(rhs) = read_buffer(other, vm) else {
            return Ok(None);
        };
        let lhs = self.get(vm.heap).as_slice();
        let len = lhs.len().saturating_add(rhs.len());
        check_repeat_size(len, 1, &vm.heap.tracker)?;
        // Reserve the complete result so a small suffix cannot double the copied prefix's capacity.
        let mut bytes = Vec::with_capacity(len);
        bytes.extend_from_slice(lhs);
        bytes.extend_from_slice(rhs);
        Ok(Some(ByteArray::allocate(bytes, vm)))
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let Some(method) = attr.static_string(vm.interns) else {
            args.drop_with(vm);
            return Err(ExcType::attribute_error(Type::ByteArray, attr.as_str(vm.interns)));
        };
        let name: &'static str = method.into();

        match method {
            StaticStrings::Clear | StaticStrings::Reverse | StaticStrings::Copy => {
                args.check_zero_args(&format!("bytearray.{name}"), vm.heap)?;
                match method {
                    StaticStrings::Clear => self.get_mut(vm.heap).0.clear(),
                    StaticStrings::Reverse => self.get_mut(vm.heap).0.reverse(),
                    _ => return self.allocate_copy(vm).map(CallResult::Value),
                }
                return Ok(CallResult::Value(Value::None));
            }
            StaticStrings::Expandtabs => return expandtabs(self, args, vm).map(CallResult::Value),
            StaticStrings::Translate => return translate(self, args, vm).map(CallResult::Value),
            StaticStrings::Maketrans => return maketrans(args, vm).map(CallResult::Value),
            StaticStrings::Fromhex => {
                let result = super::bytes::bytes_fromhex(args, vm)?;
                return Ok(CallResult::Value(convert_result(result, vm)?));
            }
            StaticStrings::Resize => {
                let value = args.get_one_arg("bytearray.resize", vm.heap)?;
                defer_drop!(value, vm);
                let size = value.as_int(vm)?;
                if size < 0 {
                    return Err(ExcType::value_error(format!(
                        "Can only resize to positive sizes, got {size}"
                    )));
                }
                let size = usize::try_from(size).map_err(|_| ExcType::overflow_index_sized_int())?;
                check_repeat_size(size, 1, &vm.heap.tracker)?;
                let data = self.get_mut(vm.heap);
                data.0
                    .try_reserve_exact(size.saturating_sub(data.0.len()))
                    .map_err(|_| ExcType::allocation_too_large())?;
                data.0.resize(size, 0);
                return Ok(CallResult::Value(Value::None));
            }
            StaticStrings::Append | StaticStrings::Extend | StaticStrings::Remove => {
                let value = args.get_one_arg(&format!("bytearray.{name}"), vm.heap)?;
                defer_drop!(value, vm);
                if method == StaticStrings::Extend {
                    if matches!(value.py_value(vm), PyValue::Int(_) | PyValue::Bool(_)) {
                        return Err(ExcType::type_error(format!(
                            "can't extend bytearray with {}",
                            value.py_type_name(vm)
                        )));
                    }
                    let bytes = collect_bytes(value, vm)?;
                    check_repeat_size(
                        self.get(vm.heap).0.len().saturating_add(bytes.len()),
                        1,
                        &vm.heap.tracker,
                    )?;
                    self.get_mut(vm.heap).0.extend(bytes);
                } else {
                    let byte = byte_value(value, vm)?;
                    if method == StaticStrings::Append {
                        let data = self.get(vm.heap);
                        vm.heap.tracker.check_growth(data.0.len(), data.0.capacity(), 1)?;
                        self.get_mut(vm.heap).0.push(byte);
                    } else {
                        let Some(index) = self.get(vm.heap).0.iter().position(|b| *b == byte) else {
                            return Err(
                                SimpleException::new_msg(ExcType::ValueError, "value not found in bytearray").into(),
                            );
                        };
                        self.get_mut(vm.heap).0.remove(index);
                    }
                }
                return Ok(CallResult::Value(Value::None));
            }
            StaticStrings::Pop | StaticStrings::Insert => {
                let values = args.into_pos_only(&format!("bytearray.{name}"), vm.heap)?;
                defer_drop!(values, vm);
                let (index, byte) = match (method, values.as_slice()) {
                    (StaticStrings::Pop, []) => (-1, None),
                    (StaticStrings::Pop, [index]) => (index.as_int(vm)?, None),
                    (StaticStrings::Insert, [index, value]) => (index.as_int(vm)?, Some(byte_value(value, vm)?)),
                    _ => {
                        return Err(ExcType::type_error(format!(
                            "bytearray.{name} received an invalid number of arguments"
                        )));
                    }
                };
                let len = i64::try_from(self.get(vm.heap).0.len()).expect("buffer length fits i64");
                let index = if index < 0 { index.saturating_add(len) } else { index };
                if let Some(byte) = byte {
                    let data = self.get(vm.heap);
                    vm.heap.tracker.check_growth(data.0.len(), data.0.capacity(), 1)?;
                    self.get_mut(vm.heap).0.insert(
                        usize::try_from(index.clamp(0, len)).expect("clamped buffer index"),
                        byte,
                    );
                    return Ok(CallResult::Value(Value::None));
                }
                if len == 0 {
                    return Err(SimpleException::new_msg(ExcType::IndexError, "pop from empty bytearray").into());
                }
                if index < 0 || index >= len {
                    return Err(SimpleException::new_msg(ExcType::IndexError, "pop index out of range").into());
                }
                return Ok(CallResult::Value(Value::Int(i64::from(
                    self.get_mut(vm.heap)
                        .0
                        .remove(usize::try_from(index).expect("checked buffer index")),
                ))));
            }
            _ => {}
        }
        let field = heap_read_ref_as_field!(self, ByteArray, 0);
        let result = super::bytes::call_bytes_method_impl(&field.as_slice(), method, args, vm)?;
        Ok(CallResult::Value(convert_result(result, vm)?))
    }
}

/// Byte-returning operations on bytearray return fresh mutable storage.
pub(crate) fn convert_result(value: Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let mut guard = DropGuard::new(value, vm);
    let (value, vm) = guard.as_parts_mut();
    let data = match value {
        Value::InternBytes(id) => {
            let bytes = vm.interns.get_bytes(*id);
            vm.heap.tracker.check_allocation(bytes.len())?;
            Some(bytes.to_vec())
        }
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Bytes(bytes) => {
                // The temporary bytes stay alive during this copy, so budget for both buffers.
                vm.heap.tracker.check_allocation(bytes.as_slice().len())?;
                Some(bytes.as_slice().to_vec())
            }
            _ => None,
        },
        _ => None,
    };
    if let Some(data) = data {
        let (value, vm) = guard.into_parts();
        value.drop_with(vm);
        return Ok(ByteArray::allocate(data, vm));
    }

    let sequence = match value {
        Value::Ref(id) => {
            let (tuple, items) = match vm.heap.get(*id) {
                HeapData::List(list) => (false, list.as_slice()),
                HeapData::Tuple(tuple) => (true, tuple.as_slice()),
                _ => {
                    let (value, _) = guard.into_parts();
                    return Ok(value);
                }
            };
            // Both the input snapshot and converted output hold one slot per item.
            vm.heap
                .tracker
                .check_allocation(items.len().saturating_mul(2 * VALUE_SIZE))?;
            Some((
                tuple,
                items.iter().map(|v| v.clone_with_heap(vm.heap)).collect::<Vec<_>>(),
            ))
        }
        _ => None,
    };
    let (value, vm) = guard.into_parts();
    let Some((tuple, items)) = sequence else {
        return Ok(value);
    };
    value.drop_with(vm);
    let iter = items.into_iter();
    defer_drop_mut!(iter, vm);
    let mut output = DropGuard::new(Vec::with_capacity(iter.len()), vm);
    for item in iter {
        let (output, vm) = output.as_parts_mut();
        output.push(convert_result(item, vm)?);
    }
    let (items, vm) = output.into_parts();
    Ok(if tuple {
        super::tuple::allocate_tuple(items.into_iter().collect(), vm.heap)
    } else {
        Value::Ref(vm.heap.allocate(HeapData::List(super::List::new(items))))
    })
}

// Limit each uninterrupted scan or fill to 64 KiB, including a single expanded tab.
const BYTE_WORK_CHUNK: usize = 64 * 1024;

#[derive(FromArgs)]
#[from_args(name = "expandtabs")]
struct ExpandtabsArgs {
    #[from_args(default = Value::Int(8))]
    tabsize: Value,
}

fn expandtabs<'h>(value: &HeapObjectRead<'h, ByteArray>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let ExpandtabsArgs { tabsize } = ExpandtabsArgs::from_args(args, vm)?;
    defer_drop!(tabsize, vm);
    let tabsize = tabsize.as_int_with_overflow(vm, ExcType::overflow_c_int)?;
    let tabsize = i32::try_from(tabsize).map_err(|_| ExcType::overflow_c_int())?;
    let tabsize = usize::try_from(tabsize.max(0)).expect("nonnegative tab size");
    let bytes = value.get(vm.heap).as_slice();
    let mut size = 0usize;
    let mut column = 0usize;
    for (index, byte) in bytes.iter().enumerate() {
        if index % BYTE_WORK_CHUNK == BYTE_WORK_CHUNK - 1 {
            vm.heap.tracker.check_time()?;
        }
        if *byte == b'\t' {
            let spaces = if tabsize == 0 { 0 } else { tabsize - column % tabsize };
            size = size.saturating_add(spaces);
            column = column.saturating_add(spaces);
        } else {
            size = size.saturating_add(1);
            column = if matches!(*byte, b'\n' | b'\r') {
                0
            } else {
                column.saturating_add(1)
            };
        }
    }
    check_repeat_size(size, 1, &vm.heap.tracker)?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(size)
        .map_err(|_| ExcType::allocation_too_large())?;
    column = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if index % BYTE_WORK_CHUNK == BYTE_WORK_CHUNK - 1 {
            vm.heap.tracker.check_time()?;
        }
        if *byte == b'\t' {
            let spaces = if tabsize == 0 { 0 } else { tabsize - column % tabsize };
            let end = result.len() + spaces;
            while end - result.len() > BYTE_WORK_CHUNK {
                result.resize(result.len() + BYTE_WORK_CHUNK, b' ');
                vm.heap.tracker.check_time()?;
            }
            result.resize(end, b' ');
            column += spaces;
        } else {
            result.push(*byte);
            column = if matches!(*byte, b'\n' | b'\r') { 0 } else { column + 1 };
        }
    }
    Ok(ByteArray::allocate(result, vm))
}

#[derive(FromArgs)]
#[from_args(name = "translate")]
struct TranslateArgs {
    #[from_args(pos_only)]
    table: Value,
    #[from_args(default)]
    delete: Option<Value>,
}

fn buffer_argument<'a>(value: &Value, vm: &'a VM<'_>) -> RunResult<&'a [u8]> {
    read_buffer(value, vm).ok_or_else(|| {
        ExcType::type_error(format!(
            "a bytes-like object is required, not '{}'",
            value.py_type_name(vm)
        ))
    })
}

fn translate<'h>(value: &HeapObjectRead<'h, ByteArray>, args: ArgValues, vm: &mut VM<'h>) -> RunResult<Value> {
    let TranslateArgs { table, delete } = TranslateArgs::from_args(args, vm)?;
    defer_drop!(table, vm);
    defer_drop!(delete, vm);
    let table = if matches!(table, Value::None) {
        None
    } else {
        Some(buffer_argument(table, vm)?)
    };
    if table.is_some_and(|table| table.len() != 256) {
        return Err(ExcType::value_error("translation table must be 256 characters long"));
    }
    let mut deleted = [false; 256];
    if let Some(delete) = delete {
        for (index, byte) in buffer_argument(delete, vm)?.iter().enumerate() {
            deleted[usize::from(*byte)] = true;
            if index % BYTE_WORK_CHUNK == BYTE_WORK_CHUNK - 1 {
                vm.heap.tracker.check_time()?;
            }
        }
    }
    let bytes = value.get(vm.heap).as_slice();
    // The source remains live, and translation can retain every byte.
    vm.heap.tracker.check_allocation(bytes.len())?;
    let mut result = Vec::with_capacity(bytes.len());
    for chunk in bytes.chunks(BYTE_WORK_CHUNK) {
        result.extend(chunk.iter().filter_map(|byte| {
            if deleted[usize::from(*byte)] {
                None
            } else {
                Some(table.map_or(*byte, |table| table[usize::from(*byte)]))
            }
        }));
        vm.heap.tracker.check_time()?;
    }
    Ok(ByteArray::allocate(result, vm))
}

pub(crate) fn maketrans(args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let (from, to) = args.get_two_args("maketrans", vm.heap)?;
    defer_drop!(from, vm);
    defer_drop!(to, vm);
    let from = buffer_argument(from, vm)?;
    let to = buffer_argument(to, vm)?;
    if from.len() != to.len() {
        return Err(ExcType::value_error("maketrans arguments must have same length"));
    }
    let mut table: Vec<u8> = (0..=255).collect();
    for (index, (from, to)) in from.iter().zip(to).enumerate() {
        table[usize::from(*from)] = *to;
        if index % BYTE_WORK_CHUNK == BYTE_WORK_CHUNK - 1 {
            vm.heap.tracker.check_time()?;
        }
    }
    Ok(super::bytes::allocate_bytes(table, vm.heap))
}

fn repeat_in_place(data: &mut Vec<u8>, count: usize) -> RunResult<()> {
    if count == 0 {
        data.clear();
        return Ok(());
    }
    if data.is_empty() || count == 1 {
        return Ok(());
    }
    let target = data
        .len()
        .checked_mul(count)
        .ok_or_else(ExcType::allocation_too_large)?;
    data.try_reserve_exact(target - data.len())
        .map_err(|_| ExcType::allocation_too_large())?;
    while data.len() < target {
        let available = data.len().min(target - data.len());
        data.extend_from_within(..available);
    }
    Ok(())
}
