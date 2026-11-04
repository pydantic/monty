//! Mutable byte storage and scoped access to its contiguous buffer.

use std::{fmt::Write, iter::successors};

use crate::{
    args::{ArgValues, FromArgs, StrArg},
    bytecode::{CallResult, VM},
    codecs::Codec,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    heap::{DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead, heap_read_ref_as_field},
    percent_format::{copy_bytes_template, percent_format_bytes},
    py_value::PyValue,
    resource_checks::check_repeat_size,
    types::{BytesIterator, CmpOrder, LazyHeapSet, PyTrait, Type, iter::checked_preallocation_hint},
    value::{EitherStr, Value, eq_bytes},
};

/// A bytearray owns its buffer; heap capabilities keep scoped borrows valid across aliases.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct ByteArray(#[serde(with = "serde_bytes")] Vec<u8>, #[serde(default)] usize);

impl ByteArray {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self(bytes, 0)
    }

    pub(crate) fn add_export(&mut self) {
        self.1 += 1;
    }

    pub(crate) fn remove_export(&mut self) {
        self.1 -= 1;
    }

    fn check_resize(&self, new_len: usize) -> RunResult<()> {
        if self.1 != 0 && self.0.len() != new_len {
            return Err(SimpleException::new_msg(
                ExcType::BufferError,
                "Existing exports of data: object cannot be re-sized",
            )
            .into());
        }
        Ok(())
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

    pub(crate) fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let InitArgs {
            source,
            encoding,
            errors,
        } = InitArgs::from_args(args, vm)?;
        defer_drop!(source, vm);
        defer_drop!(encoding, vm);
        defer_drop!(errors, vm);
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
                let count = usize::try_from(count).map_err(|_| ExcType::overflow_index_sized_int())?;
                check_repeat_size(count, 1, &vm.heap.tracker)?;
                let mut data = Vec::new();
                data.try_reserve_exact(count)
                    .map_err(|_| ExcType::allocation_too_large())?;
                data.resize(count, 0);
                data
            }
            (None, None) => Vec::new(),
            (Some(value), None) => {
                if !value.py_is_iterable(vm) && read_buffer(value, vm).is_none() {
                    return Err(ExcType::type_error(format!(
                        "cannot convert '{}' object to bytearray",
                        value.py_type_name(vm)
                    )));
                }
                collect_bytes(value, vm)?
            }
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
    if let Value::Ref(id) = value
        && let HeapData::MemoryView(view) = vm.heap.get(*id)
    {
        return view.to_vec(vm);
    }
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
            HeapData::MemoryView(view) => view.contiguous(vm).ok(),
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
        Ok(Some(convert_result(value, vm)))
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
            Ok(convert_result(value, vm))
        } else {
            let index = key.as_index(vm, Type::ByteArray)?;
            let byte = super::bytes::get_byte_at_index(self.get(vm.heap).as_slice(), index)
                .ok_or_else(|| SimpleException::new_msg(ExcType::IndexError, "bytearray index out of range"))?;
            Ok(Value::Int(i64::from(byte)))
        }
    }

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
            let replacement = collect_bytes(value, vm)?;
            // User iteration can mutate the destination, so normalize against its final length.
            let (start, stop, step) = slice.indices(self.get(vm.heap).0.len())?;
            if step == 1 {
                let start = usize::try_from(start).expect("normalized slice start");
                let stop = (usize::try_from(stop).expect("normalized slice stop")).max(start);
                let new_len = self.get(vm.heap).0.len() - (stop - start) + replacement.len();
                self.get(vm.heap).check_resize(new_len)?;
                check_repeat_size(new_len, 1, &vm.heap.tracker)?;
                self.get_mut(vm.heap).0.splice(start..stop, replacement);
            } else {
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
        self.get(vm.heap)
            .check_resize(self.get(vm.heap).0.len().saturating_add(bytes.len()))?;
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
        let Some(count) = repetition_count(other, vm)? else {
            return Ok(false);
        };
        self.get(vm.heap)
            .check_resize(self.get(vm.heap).0.len().saturating_mul(count))?;
        check_repeat_size(self.get(vm.heap).0.len(), count, &vm.heap.tracker)?;
        repeat_in_place(&mut self.get_mut(vm.heap).0, count)?;
        Ok(true)
    }

    fn py_mul_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let Some(count) = repetition_count(other, vm)? else {
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
        check_repeat_size(lhs.len().saturating_add(rhs.len()), 1, &vm.heap.tracker)?;
        let mut bytes = lhs.to_vec();
        bytes.extend_from_slice(rhs);
        Ok(Some(ByteArray::allocate(bytes, vm)))
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let method = attr.as_str(vm.interns).to_owned();
        let args = validate_method_args(&method, args, vm)?;
        match method.as_str() {
            "clear" | "reverse" | "copy" => {
                args.check_zero_args(&format!("bytearray.{method}"), vm.heap)?;
                match method.as_str() {
                    "clear" => {
                        self.get(vm.heap).check_resize(0)?;
                        self.get_mut(vm.heap).0.clear();
                    }
                    "reverse" => self.get_mut(vm.heap).0.reverse(),
                    _ => return self.allocate_copy(vm).map(CallResult::Value),
                }
                return Ok(CallResult::Value(Value::None));
            }
            "hex" => {
                let field = heap_read_ref_as_field!(self, ByteArray, 0);
                return super::bytes::bytes_hex(&field.as_slice(), args, vm).map(CallResult::Value);
            }
            "expandtabs" => return expandtabs(self, args, vm).map(CallResult::Value),
            "translate" => return translate(self, args, vm).map(CallResult::Value),
            "maketrans" => return maketrans(args, vm).map(CallResult::Value),
            "fromhex" => {
                let result = super::bytes::bytes_fromhex(args, vm)?;
                return Ok(CallResult::Value(convert_result(result, vm)));
            }
            "resize" => {
                let value = args.get_one_arg("bytearray.resize", vm.heap)?;
                defer_drop!(value, vm);
                let size = value.as_int(vm)?;
                if size < 0 {
                    return Err(ExcType::value_error(format!(
                        "Can only resize to positive sizes, got {size}"
                    )));
                }
                let size = usize::try_from(size).map_err(|_| ExcType::overflow_index_sized_int())?;
                self.get(vm.heap).check_resize(size)?;
                check_repeat_size(size, 1, &vm.heap.tracker)?;
                let data = self.get_mut(vm.heap);
                data.0
                    .try_reserve_exact(size.saturating_sub(data.0.len()))
                    .map_err(|_| ExcType::allocation_too_large())?;
                data.0.resize(size, 0);
                return Ok(CallResult::Value(Value::None));
            }
            "append" | "extend" | "remove" => {
                let value = args.get_one_arg(&format!("bytearray.{method}"), vm.heap)?;
                defer_drop!(value, vm);
                if method == "extend" {
                    if let Value::Ref(id) = value
                        && let HeapData::MemoryView(view) = vm.heap.get(*id)
                    {
                        view.check_live()?;
                        if view.contiguous(vm).is_err() {
                            return Err(ExcType::type_error("can\'t set bytearray slice from memoryview"));
                        }
                    }
                    if matches!(value.py_value(vm), PyValue::Int(_) | PyValue::Bool(_)) {
                        return Err(ExcType::type_error(format!(
                            "can't extend bytearray with {}",
                            value.py_type_name(vm)
                        )));
                    }
                    let bytes = collect_bytes(value, vm)?;
                    self.get(vm.heap)
                        .check_resize(self.get(vm.heap).0.len().saturating_add(bytes.len()))?;
                    check_repeat_size(
                        self.get(vm.heap).0.len().saturating_add(bytes.len()),
                        1,
                        &vm.heap.tracker,
                    )?;
                    self.get_mut(vm.heap).0.extend(bytes);
                } else {
                    let byte = byte_value(value, vm)?;
                    if method == "append" {
                        self.get(vm.heap)
                            .check_resize(self.get(vm.heap).0.len().saturating_add(1))?;
                        let data = self.get(vm.heap);
                        vm.heap.tracker.check_growth(data.0.len(), data.0.capacity(), 1)?;
                        self.get_mut(vm.heap).0.push(byte);
                    } else {
                        let Some(index) = self.get(vm.heap).0.iter().position(|b| *b == byte) else {
                            return Err(
                                SimpleException::new_msg(ExcType::ValueError, "value not found in bytearray").into(),
                            );
                        };
                        self.get(vm.heap).check_resize(self.get(vm.heap).0.len() - 1)?;
                        self.get_mut(vm.heap).0.remove(index);
                    }
                }
                return Ok(CallResult::Value(Value::None));
            }
            "pop" | "insert" => {
                let values = args.into_pos_only(&format!("bytearray.{method}"), vm.heap)?;
                defer_drop!(values, vm);
                let (index, byte) = match (method.as_str(), values.as_slice()) {
                    ("pop", []) => (-1, None),
                    ("pop", [index]) => (index.as_int(vm)?, None),
                    ("insert", [index, value]) => (index.as_int(vm)?, Some(byte_value(value, vm)?)),
                    _ => {
                        unreachable!("method argument counts were validated before dispatch");
                    }
                };
                let len = i64::try_from(self.get(vm.heap).0.len()).expect("buffer length fits i64");
                let index = if index < 0 { index.saturating_add(len) } else { index };
                if let Some(byte) = byte {
                    self.get(vm.heap)
                        .check_resize(self.get(vm.heap).0.len().saturating_add(1))?;
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
                self.get(vm.heap).check_resize(self.get(vm.heap).0.len() - 1)?;
                return Ok(CallResult::Value(Value::Int(i64::from(
                    self.get_mut(vm.heap)
                        .0
                        .remove(usize::try_from(index).expect("checked buffer index")),
                ))));
            }
            _ => {}
        }
        let Some(method) = attr.static_string(vm.interns) else {
            args.drop_with(vm);
            return Err(ExcType::attribute_error(Type::ByteArray, &method));
        };
        let field = heap_read_ref_as_field!(self, ByteArray, 0);
        let result = super::bytes::call_bytes_method_impl(&field.as_slice(), Type::ByteArray, method, args, vm)?;
        Ok(CallResult::Value(convert_result(result, vm)))
    }
}

/// Byte-returning operations on bytearray return fresh mutable storage.
pub(crate) fn convert_result(value: Value, vm: &mut VM<'_>) -> Value {
    let data = match &value {
        Value::InternBytes(id) => Some(vm.interns.get_bytes(*id).to_vec()),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Bytes(bytes) => Some(bytes.as_slice().to_vec()),
            _ => None,
        },
        _ => None,
    };
    if let Some(data) = data {
        value.drop_with(vm);
        ByteArray::allocate(data, vm)
    } else {
        let sequence = match &value {
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::List(list) => Some((
                    false,
                    list.as_slice()
                        .iter()
                        .map(|v| v.clone_with_heap(vm.heap))
                        .collect::<Vec<_>>(),
                )),
                HeapData::Tuple(tuple) => Some((
                    true,
                    tuple
                        .as_slice()
                        .iter()
                        .map(|v| v.clone_with_heap(vm.heap))
                        .collect::<Vec<_>>(),
                )),
                _ => None,
            },
            _ => None,
        };
        if let Some((tuple, items)) = sequence {
            value.drop_with(vm);
            let items = items.into_iter().map(|item| convert_result(item, vm));
            if tuple {
                super::tuple::allocate_tuple(items.collect(), vm.heap)
            } else {
                let list = super::List::new(items.collect());
                Value::Ref(vm.heap.allocate(HeapData::List(list)))
            }
        } else {
            value
        }
    }
}

#[derive(FromArgs)]
#[from_args(name = "expandtabs", at_most_total)]
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
    for byte in bytes {
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
    for byte in bytes {
        if *byte == b'\t' {
            let spaces = if tabsize == 0 { 0 } else { tabsize - column % tabsize };
            result.resize(result.len() + spaces, b' ');
            column += spaces;
        } else {
            result.push(*byte);
            column = if matches!(*byte, b'\n' | b'\r') { 0 } else { column + 1 };
        }
    }
    Ok(ByteArray::allocate(result, vm))
}

#[derive(FromArgs)]
#[from_args(name = "translate", at_most_total)]
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
        for byte in buffer_argument(delete, vm)? {
            deleted[usize::from(*byte)] = true;
        }
    }
    let result = value
        .get(vm.heap)
        .as_slice()
        .iter()
        .filter_map(|byte| {
            if deleted[usize::from(*byte)] {
                None
            } else {
                Some(table.map_or(*byte, |table| table[usize::from(*byte)]))
            }
        })
        .collect();
    Ok(ByteArray::allocate(result, vm))
}

pub(crate) fn maketrans(args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let args = validate_method_args("maketrans", args, vm)?;
    let (from, to) = args.get_two_args("maketrans", vm.heap)?;
    defer_drop!(from, vm);
    defer_drop!(to, vm);
    let from = buffer_argument(from, vm)?;
    let to = buffer_argument(to, vm)?;
    if from.len() != to.len() {
        return Err(ExcType::value_error("maketrans arguments must have same length"));
    }
    let mut table: Vec<u8> = (0..=255).collect();
    for (from, to) in from.iter().zip(to) {
        table[usize::from(*from)] = *to;
    }
    Ok(super::bytes::allocate_bytes(table, vm.heap))
}

/// CPython uses qualified method names for clinic errors and bare names for unpack errors.
fn validate_method_args(method: &str, args: ArgValues, vm: &mut VM<'_>) -> RunResult<ArgValues> {
    let qualified = format!("bytearray.{method}");
    let (min, max, exact) = match method {
        "lower" | "upper" | "capitalize" | "title" | "swapcase" | "isalpha" | "isdigit" | "isalnum" | "isspace"
        | "islower" | "isupper" | "isascii" | "istitle" | "clear" | "reverse" | "copy" => {
            args.check_zero_args(&qualified, vm.heap)?;
            return Ok(ArgValues::Empty);
        }
        "append" | "extend" | "remove" | "resize" | "join" | "fromhex" | "removeprefix" | "removesuffix"
        | "partition" | "rpartition" | "zfill" => (1, 1, true),
        "insert" | "maketrans" => (2, 2, true),
        "pop" | "strip" | "lstrip" | "rstrip" => (0, 1, false),
        "find" | "rfind" | "index" | "rindex" | "count" | "startswith" | "endswith" => (1, 3, false),
        "center" | "ljust" | "rjust" => (1, 2, false),
        "replace" => (2, 3, false),
        "translate" if args.count() == 0 => {
            args.drop_with(vm);
            return Err(ExcType::type_error_at_least_positional("translate", 1, 0));
        }
        _ => return Ok(args),
    };
    let args = args.reject_kwargs(&qualified, vm.heap)?;
    let count = args.count();
    let error = if exact && count != min {
        Some(ExcType::type_error_arg_count(
            if min == 1 { &qualified } else { method },
            min,
            count,
        ))
    } else if count < min {
        Some(ExcType::type_error_at_least(method, min, count))
    } else if count > max {
        Some(ExcType::type_error_at_most(method, max, count))
    } else {
        None
    };
    if let Some(error) = error {
        args.drop_with(vm);
        Err(error)
    } else {
        Ok(args)
    }
}

pub(crate) fn fromhex(args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let args = validate_method_args("fromhex", args, vm)?;
    let value = super::bytes::bytes_fromhex(args, vm)?;
    Ok(convert_result(value, vm))
}

fn repetition_count(value: &Value, vm: &mut VM<'_>) -> RunResult<Option<usize>> {
    let Some(count) = value.py_index_impl(vm)? else {
        return Ok(None);
    };
    defer_drop!(count, vm);
    let count = count.as_int(vm)?;
    usize::try_from(count.max(0))
        .map(Some)
        .map_err(|_| ExcType::overflow_index_sized_int().into())
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
