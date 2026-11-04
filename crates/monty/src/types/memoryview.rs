//! Shared, scoped views of byte buffers. Views retain owners rather than storage pointers.

use std::{fmt::Write, mem};

use monty_types::ResourceTracker;

mod format;
mod layout;
use format::BufferFormat;

use crate::{
    args::{ArgValues, FromArgs, StrArg},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    hash::{HashValue, hash_python_bytes},
    heap::{DropGuard, DropWithContext, Heap, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReader},
    py_value::PyValue,
    resource_checks::check_repeat_size,
    types::{PyTrait, Type, instance::class_defines},
    value::{EitherStr, VALUE_SIZE, Value},
};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct MemoryView {
    owner: Value,
    start: i64,
    step: i64,
    len: usize,
    readonly: bool,
    format: char,
    native_prefix: bool,
    #[serde(default)]
    cached_hash: Option<HashValue>,
    #[serde(default)]
    shape: Option<Vec<usize>>,
    #[serde(default)]
    strides: Vec<i64>,
}

impl MemoryView {
    pub(crate) fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let owner = args.get_one_arg("memoryview", vm.heap)?;
        defer_drop!(owner, vm);
        let view = if let Value::Ref(id) = owner
            && let HeapData::MemoryView(view) = vm.heap.get(*id)
        {
            view.check_live()?;
            Self {
                owner: view.owner.clone_with_heap(vm.heap),
                start: view.start,
                step: view.step,
                len: view.len,
                readonly: view.readonly,
                format: view.format,
                native_prefix: view.native_prefix,
                cached_hash: None,
                shape: view.shape.clone(),
                strides: view.strides.clone(),
            }
        } else {
            let bytes = super::bytearray::read_buffer(owner, vm).ok_or_else(|| {
                ExcType::type_error(format!(
                    "memoryview: a bytes-like object is required, not '{}'",
                    owner.py_type_name(vm)
                ))
            })?;
            let len = bytes.len();
            let readonly = !matches!(owner, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::ByteArray(_)));
            Self {
                owner: owner.clone_with_heap(vm.heap),
                start: 0,
                step: 1,
                len,
                readonly,
                format: 'B',
                native_prefix: false,
                cached_hash: None,
                shape: None,
                strides: Vec::new(),
            }
        };
        Ok(Self::allocate(view, vm))
    }

    fn allocate(view: Self, vm: &mut VM<'_>) -> Value {
        if let Value::Ref(id) = &view.owner
            && let Some(owner) = vm.heap.read_as::<super::ByteArray>(*id)
        {
            owner.get_mut(vm.heap).add_export();
        }
        Value::Ref(vm.heap.allocate(HeapData::MemoryView(Box::new(view))))
    }

    pub(crate) fn check_live(&self) -> RunResult<()> {
        if matches!(self.owner, Value::None) {
            Err(ExcType::value_error(
                "operation forbidden on released memoryview object",
            ))
        } else {
            Ok(())
        }
    }

    pub(crate) fn owner_id(&self) -> Option<HeapId> {
        if let Value::Ref(id) = self.owner {
            Some(id)
        } else {
            None
        }
    }

    // Called before the owned reference is decremented, including implicit heap cleanup.
    pub(crate) fn remove_export(&self, heap: &mut Heap) {
        if let Some(id) = self.owner_id() {
            HeapReader::with(heap, &mut (), |heap, ()| {
                if let Some(owner) = heap.read_as::<super::ByteArray>(id) {
                    owner.get_mut(heap).remove_export();
                }
            });
        }
    }
}

impl HeapItem for MemoryView {
    fn py_dec_ref_ids(&mut self, ids: &mut Vec<HeapId>) {
        self.owner.py_dec_ref_ids(ids);
    }
}

impl<'h> HeapObjectRead<'h, MemoryView> {
    fn release(&self, vm: &mut VM<'h>) {
        if let Some(id) = self.get(vm.heap).owner_id()
            && let Some(owner) = vm.heap.read_as::<super::ByteArray>(id)
        {
            owner.get_mut(vm.heap).remove_export();
        }
        let owner = mem::replace(&mut self.get_mut(vm.heap).owner, Value::None);
        owner.drop_with(vm);
    }

    fn subview(&self, start: i64, step: i64, len: usize, readonly: bool, format: char, vm: &mut VM<'h>) -> Value {
        let data = self.get(vm.heap);
        let view = MemoryView {
            owner: data.owner.clone_with_heap(vm.heap),
            start,
            step,
            len,
            readonly,
            format,
            native_prefix: data.native_prefix,
            cached_hash: None,
            shape: data.shape.clone(),
            strides: data.strides.clone(),
        };
        MemoryView::allocate(view, vm)
    }
    fn scalar_index(&self, key: &Value, assignment: bool, vm: &mut VM<'h>) -> RunResult<usize> {
        let ndim = self.get(vm.heap).ndim();
        let tuple = if let Value::Ref(id) = key
            && let HeapData::Tuple(tuple) = vm.heap.get(*id)
        {
            if tuple.as_slice().len() > 64 {
                check_repeat_size(tuple.as_slice().len(), VALUE_SIZE + 8, &vm.heap.tracker)?;
            }
            Some(
                tuple
                    .as_slice()
                    .iter()
                    .map(|value| value.clone_with_heap(vm.heap))
                    .collect::<Vec<_>>(),
            )
        } else {
            None
        };
        defer_drop!(tuple, vm);
        if ndim == 0 {
            if matches!(key, Value::Ellipsis) || tuple.as_ref().is_some_and(Vec::is_empty) {
                return Ok(0);
            }
            return Err(ExcType::type_error("invalid indexing of 0-dim memory"));
        }
        if let Some(values) = tuple {
            let slice_count = values
                .iter()
                .filter(|value| matches!(value, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Slice(_))))
                .count();
            if slice_count != 0 {
                if slice_count != values.len() {
                    return Err(invalid_key());
                }
                return Err(not_implemented(if assignment {
                    "memoryview slice assignments are currently restricted to ndim = 1"
                } else {
                    "multi-dimensional slicing is not implemented"
                }));
            }
            // Validate key kinds and tuple length before invoking any index callbacks.
            if !values.iter().all(|value| is_index_key(value, vm)) {
                return Err(invalid_key());
            }
            if values.len() != ndim {
                if values.len() < ndim {
                    return Err(not_implemented("sub-views are not implemented"));
                }
                return Err(ExcType::type_error(format!(
                    "cannot index {ndim}-dimension view with {}-element tuple",
                    values.len()
                )));
            }
            let mut index = 0usize;
            for (axis, value) in values.iter().enumerate() {
                let coordinate = coerce_index(value, vm)?;
                let length = self.get(vm.heap).dimension(axis);
                let coordinate = normalize_index(coordinate, length, axis)?;
                self.get(vm.heap).check_live()?;
                index = index
                    .checked_mul(length)
                    .and_then(|i| i.checked_add(coordinate))
                    .expect("coordinate index");
            }
            return Ok(index);
        }
        if assignment && ndim > 1 {
            return Err(if is_index_key(key, vm) {
                not_implemented("sub-views are not implemented")
            } else {
                invalid_key()
            });
        }
        let index = coerce_index(key, vm)?;
        if assignment {
            let index = normalize_index(index, self.get(vm.heap).len, 0)?;
            self.get(vm.heap).check_live()?;
            return Ok(index);
        }
        self.get(vm.heap).check_live()?;
        if ndim > 1 {
            return Err(not_implemented("multi-dimensional sub-views are not implemented"));
        }
        normalize_index(index, self.get(vm.heap).len, 0)
    }

    fn cast_view(&self, format: &StrArg, shape_value: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Value> {
        self.get(vm.heap).check_live()?;
        let source = self.get(vm.heap);
        if !source.is_contiguous(false) {
            return Err(ExcType::type_error(
                "memoryview: casts are restricted to C-contiguous views",
            ));
        }
        if (shape_value.is_some() || source.ndim() > 1) && source.len == 0 {
            return Err(ExcType::type_error(
                "memoryview: cannot cast view with zeros in shape or strides",
            ));
        }
        let raw_shape = shape_value.map(|shape| shape_items(shape, vm)).transpose()?;
        if raw_shape.is_some_and(|shape| shape.len() > 64) {
            return Err(ExcType::value_error(
                "memoryview: number of dimensions must not exceed 64",
            ));
        }
        if raw_shape.is_some_and(|shape| source.ndim() != 1 && shape.len() != 1) {
            return Err(ExcType::type_error("memoryview: cast must be 1D -> ND or ND -> 1D"));
        }
        let target = BufferFormat::parse(format.as_str(vm))?;
        let native_prefix = format.as_str(vm).starts_with('@');
        if !matches!(source.format, 'B' | 'b' | 'c') && !matches!(target.code(), 'B' | 'b' | 'c') {
            return Err(ExcType::type_error(
                "memoryview: cannot cast between two non-byte formats",
            ));
        }
        let nbytes = source.nbytes();
        if !nbytes.is_multiple_of(target.itemsize()) {
            return Err(ExcType::type_error("memoryview: length is not a multiple of itemsize"));
        }
        let len = nbytes / target.itemsize();
        let shape = if let Some(items) = raw_shape {
            let mut shape = Vec::with_capacity(items.len());
            let mut product = 1u64;
            for item in items {
                let dimension = match item.py_value(vm) {
                    PyValue::Bool(value) => i64::from(value),
                    PyValue::Int(integer) => integer.to_i64(vm).ok_or_else(|| {
                        SimpleException::new_msg(ExcType::OverflowError, "Python int too large to convert to C ssize_t")
                    })?,
                    _ => {
                        return Err(ExcType::type_error(
                            "memoryview.cast(): elements of shape must be integers",
                        ));
                    }
                };
                if dimension <= 0 {
                    return Err(ExcType::value_error(
                        "memoryview.cast(): elements of shape must be integers > 0",
                    ));
                }
                let dimension = u64::try_from(dimension).expect("positive dimension");
                product = product
                    .checked_mul(dimension)
                    .filter(|product| i64::try_from(*product).is_ok())
                    .ok_or_else(|| ExcType::value_error("memoryview.cast(): product(shape) > SSIZE_MAX"))?;
                shape.push(dimension);
            }
            if product != u64::try_from(len).expect("element count") {
                return Err(ExcType::type_error(
                    "memoryview: product(shape) * itemsize != buffer size",
                ));
            }
            Some(
                shape
                    .into_iter()
                    .map(|dimension| usize::try_from(dimension).expect("admitted dimension"))
                    .collect::<Vec<_>>(),
            )
        } else {
            None
        };
        let mut strides = Vec::new();
        if let Some(shape) = &shape {
            strides.resize(shape.len(), 0);
            let mut stride = i64::try_from(target.itemsize()).expect("item size");
            for axis in (0..shape.len()).rev() {
                strides[axis] = stride;
                stride = stride
                    .checked_mul(i64::try_from(shape[axis]).expect("shape dimension"))
                    .expect("admitted cast shape");
            }
        }
        let source = self.get(vm.heap);
        let view = MemoryView {
            owner: source.owner.clone_with_heap(vm.heap),
            start: source.start,
            step: strides
                .first()
                .copied()
                .unwrap_or(i64::try_from(target.itemsize()).expect("item size")),
            len,
            readonly: source.readonly,
            format: target.code(),
            native_prefix,
            cached_hash: None,
            shape,
            strides,
        };
        Ok(MemoryView::allocate(view, vm))
    }

    fn list_level(&self, axis: usize, start: usize, vm: &mut VM<'h>) -> RunResult<Value> {
        let ndim = self.get(vm.heap).ndim();
        if ndim == 0 {
            return self.get(vm.heap).item(0, vm);
        }
        let mut recursion = vm.recursion_guard()?;
        let vm = &mut *recursion;
        let length = self.get(vm.heap).dimension(axis);
        let inner = (axis + 1..ndim)
            .map(|dimension| self.get(vm.heap).dimension(dimension))
            .product::<usize>();
        let mut guard = DropGuard::new(Vec::with_capacity(length), vm);
        let (values, vm) = guard.as_parts_mut();
        for index in 0..length {
            let index = start + index * inner;
            let value = if axis + 1 == ndim {
                self.get(vm.heap).item(index, vm)?
            } else {
                self.list_level(axis + 1, index, vm)?
            };
            values.push(value);
            if axis + 1 == ndim {
                vm.heap.tracker.check_time_every(index)?;
            }
        }
        let (values, vm) = guard.into_parts();
        Ok(Value::Ref(vm.heap.allocate(HeapData::List(super::List::new(values)))))
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, MemoryView> {
    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::MemoryView
    }
    fn py_len(&self, vm: &VM<'h>) -> Option<usize> {
        let data = self.get(vm.heap);
        (data.ndim() != 0).then(|| data.dimension(0))
    }
    fn py_bool(&self, vm: &mut VM<'h>) -> RunResult<bool> {
        Ok(self.get(vm.heap).checked_len()? != 0)
    }
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _: &mut super::LazyHeapSet) -> RunResult<()> {
        f.write_str(if self.get(vm.heap).check_live().is_ok() {
            "<memory at 0x0>"
        } else {
            "<released memory at 0x0>"
        })?;
        Ok(())
    }
    fn py_is_context_manager(&self, _: &VM<'h>) -> bool {
        true
    }
    fn py_enter(&mut self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        self.get(vm.heap).check_live()?;
        vm.heap.inc_ref(self.id());
        Ok(CallResult::Value(Value::Ref(self.id())))
    }
    fn py_exit(&mut self, vm: &mut VM<'h>, _: Option<HeapId>) -> RunResult<CallResult> {
        self.release(vm);
        Ok(CallResult::Value(Value::None))
    }
    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        let data = self.get(vm.heap);
        if let Some(hash) = data.cached_hash {
            return Ok(Some(hash));
        }
        data.check_live()?;
        if !data.readonly {
            return Err(ExcType::value_error("cannot hash writable memoryview object"));
        }
        if !matches!(data.format, 'B' | 'b' | 'c') {
            return Err(ExcType::value_error(
                "memoryview: hashing is restricted to formats 'B', 'b' or 'c'",
            ));
        }
        if let Value::Ref(id) = data.owner
            && matches!(vm.heap.get(id), HeapData::ByteArray(_))
        {
            return Err(ExcType::type_error("unhashable type: 'bytearray'"));
        }
        let hash = if data.is_contiguous(false) {
            hash_python_bytes(data.contiguous(vm)?)
        } else {
            hash_python_bytes(&data.to_vec(vm)?)
        };
        self.get_mut(vm.heap).cached_hash = Some(hash);
        Ok(Some(hash))
    }
    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        if self.get(vm.heap).check_live().is_err() {
            return Ok(Some(matches!(other, Value::Ref(id) if *id == self.id())));
        }
        if let Value::Ref(id) = other
            && let Some(other) = vm.heap.read_as::<MemoryView>(*id)
        {
            if other.get(vm.heap).check_live().is_err() {
                return Ok(Some(false));
            }
            if self.get(vm.heap).shape_values() != other.get(vm.heap).shape_values() {
                return Ok(Some(false));
            }
            for i in 0..self.get(vm.heap).len {
                let a = self.get(vm.heap).item(i, vm)?;
                defer_drop!(a, vm);
                let b = other.get(vm.heap).item(i, vm)?;
                defer_drop!(b, vm);
                if !a.py_eq_operator(b, vm)? {
                    return Ok(Some(false));
                }
                vm.heap.tracker.check_time_every(i)?;
            }
            return Ok(Some(true));
        }
        let Some(bytes) = super::bytearray::read_buffer(other, vm) else {
            return Ok(None);
        };
        if self.get(vm.heap).ndim() != 1 || self.get(vm.heap).len != bytes.len() {
            return Ok(Some(false));
        }
        for i in 0..self.get(vm.heap).len {
            let byte = Value::Int(i64::from(super::bytearray::read_buffer(other, vm).expect("buffer")[i]));
            let item = self.get(vm.heap).item(i, vm)?;
            defer_drop!(item, vm);
            if !item.py_eq_operator(&byte, vm)? {
                return Ok(Some(false));
            }
            vm.heap.tracker.check_time_every(i)?;
        }
        Ok(Some(true))
    }
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let data = self.get(vm.heap);
        data.check_live()?;
        let c_contiguous = data.is_contiguous(false);
        let f_contiguous = data.is_contiguous(true);
        let value = match attr.as_str(vm.interns) {
            "obj" => data.owner.clone_with_heap(vm.heap),
            "nbytes" => Value::Int(i64::try_from(data.nbytes()).expect("view size")),
            "readonly" => Value::Bool(data.readonly),
            "itemsize" => Value::Int(i64::try_from(data.itemsize()).expect("item size")),
            "ndim" => Value::Int(i64::try_from(data.ndim()).expect("dimensions")),
            "format" => super::allocate_string(data.format_name(), vm.heap),
            "shape" => super::tuple::allocate_tuple(
                data.shape_values()
                    .into_iter()
                    .map(|v| Value::Int(i64::try_from(v).expect("dimension")))
                    .collect(),
                vm.heap,
            ),
            "strides" => {
                super::tuple::allocate_tuple(data.stride_values().into_iter().map(Value::Int).collect(), vm.heap)
            }
            "suboffsets" => super::tuple::allocate_tuple(vec![].into(), vm.heap),
            "c_contiguous" => Value::Bool(c_contiguous),
            "f_contiguous" => Value::Bool(f_contiguous),
            "contiguous" => Value::Bool(c_contiguous || f_contiguous),
            _ => return Ok(None),
        };
        Ok(Some(CallResult::Value(value)))
    }
    fn py_getitem(&self, key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        self.get(vm.heap).check_live()?;
        if self.get(vm.heap).ndim() == 0 && matches!(key, Value::Ellipsis) {
            vm.heap.inc_ref(self.id());
            return Ok(Value::Ref(self.id()));
        }
        if let Value::Ref(id) = key
            && let HeapData::Slice(slice) = vm.heap.get(*id)
        {
            if self.get(vm.heap).ndim() == 0 {
                return Err(ExcType::type_error("invalid indexing of 0-dim memory"));
            }
            let (start, stop, step) = slice.indices(self.get(vm.heap).checked_len()?)?;
            // PySlice_Unpack reserves SSIZE_MIN, so native buffer slicing clamps it to -SSIZE_MAX.
            let step = step.max(-i64::MAX);
            let axis_len = slice_length(start, stop, step);
            let data = self.get(vm.heap);
            let inner = (1..data.ndim()).map(|axis| data.dimension(axis)).product::<usize>();
            let total = axis_len.checked_mul(inner).expect("slice element count");
            let offset = if total == 0 {
                0
            } else {
                start
                    .checked_mul(data.stride(0))
                    .and_then(|offset| data.start.checked_add(offset))
                    .ok_or_else(ExcType::overflow_index_sized_int)?
            };
            let stride = if axis_len <= 1 {
                data.stride(0).wrapping_mul(step)
            } else {
                data.stride(0)
                    .checked_mul(step)
                    .ok_or_else(ExcType::overflow_index_sized_int)?
            };
            let result = self.subview(offset, stride, total, data.readonly, data.format, vm);
            if let Value::Ref(id) = &result {
                let view = vm.heap.read_as::<MemoryView>(*id).expect("slice view");
                if let Some(shape) = &mut view.get_mut(vm.heap).shape {
                    shape[0] = axis_len;
                }
                if !view.get(vm.heap).strides.is_empty() {
                    view.get_mut(vm.heap).strides[0] = stride;
                }
            }
            return Ok(result);
        }
        let index = self.scalar_index(key, false, vm)?;
        self.get(vm.heap).item(index, vm)
    }

    fn py_setitem(&mut self, key: Value, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        defer_drop!(key, vm);
        defer_drop!(value, vm);
        self.get(vm.heap).check_live()?;
        if self.get(vm.heap).readonly {
            return Err(ExcType::type_error("cannot modify read-only memory"));
        }
        let slice = if let Value::Ref(id) = key
            && let HeapData::Slice(slice) = vm.heap.get(*id)
        {
            Some(slice.clone())
        } else {
            None
        };
        if let Some(slice) = slice {
            if self.get(vm.heap).ndim() != 1 {
                if self.get(vm.heap).ndim() == 0 {
                    return Err(ExcType::type_error("invalid indexing of 0-dim memory"));
                }
                return Err(not_implemented(
                    "memoryview slice assignments are currently restricted to ndim = 1",
                ));
            }
            let (start, stop, step) = slice.indices(self.get(vm.heap).len)?;
            let count = slice_length(start, stop, step);
            let (rhs_format, bytes) = if let Value::Ref(id) = value
                && let HeapData::MemoryView(other) = vm.heap.get(*id)
            {
                other.check_live()?;
                if other.ndim() != 1 {
                    return Err(structure_error());
                }
                (other.format, other.to_vec(vm)?)
            } else {
                let source = super::bytearray::read_buffer(value, vm).ok_or_else(|| {
                    ExcType::type_error(format!(
                        "a bytes-like object is required, not '{}'",
                        value.py_type_name(vm)
                    ))
                })?;
                check_repeat_size(source.len(), 1, &vm.heap.tracker)?;
                ('B', source.to_vec())
            };
            let data = self.get(vm.heap);
            if data.format != rhs_format || bytes.len() != count.checked_mul(data.itemsize()).expect("slice bytes") {
                return Err(structure_error());
            }
            let view_start = data.start;
            let view_step = data.stride(0);
            let size = data.itemsize();
            let id = data.owner_id().expect("writable owner");
            let owner = vm.heap.read_as::<super::ByteArray>(id).expect("bytearray owner");
            // End each mutable buffer borrow before polling the execution clock.
            let batch_size = ResourceTracker::LOOP_CHECK_INTERVAL;
            for (batch_index, batch) in bytes.chunks(size * batch_size).enumerate() {
                let target = owner.get_mut(vm.heap).as_mut_slice();
                for (relative, chunk) in batch.chunks_exact(size).enumerate() {
                    let index = batch_index * batch_size + relative;
                    let logical = i128::from(start) + i128::try_from(index).expect("view index") * i128::from(step);
                    let offset = i128::from(view_start) + logical * i128::from(view_step);
                    let offset = usize::try_from(offset).expect("normalized view offset");
                    target[offset..offset + size].copy_from_slice(chunk);
                }
                vm.heap
                    .tracker
                    .check_time_every(batch_index * batch_size + batch.len() / size - 1)?;
            }
            return Ok(());
        }
        let index = self.scalar_index(key, true, vm)?;
        let format = self.get(vm.heap).scalar_format();
        let bytes = format.pack(value, self, vm)?;
        self.get(vm.heap).check_live()?;
        let data = self.get(vm.heap);
        let offset = data.offset(index);
        let id = data.owner_id().expect("writable owner");
        let owner = vm.heap.read_as::<super::ByteArray>(id).expect("bytearray owner");
        owner.get_mut(vm.heap).as_mut_slice()[offset..offset + bytes.len()].copy_from_slice(&bytes);
        Ok(())
    }
    fn py_contains_impl(&self, _: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        self.get(vm.heap).check_live()?;
        if self.get(vm.heap).ndim() == 0 {
            return Err(ExcType::type_error(
                "argument of type 'memoryview' is not a container or iterable",
            ));
        }
        Ok(None)
    }
    fn py_is_iterable(&self, _: &VM<'h>) -> bool {
        true
    }
    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        self.get(vm.heap).check_live()?;
        if self.get(vm.heap).ndim() == 0 {
            return Err(ExcType::type_error("invalid indexing of 0-dim memory"));
        }
        if self.get(vm.heap).ndim() > 1 {
            return Err(not_implemented("multi-dimensional sub-views are not implemented"));
        }
        vm.heap.inc_ref(self.id());
        Ok(CallResult::Value(Value::Ref(vm.heap.allocate(
            HeapData::MemoryViewIterator(MemoryViewIterator {
                view: Value::Ref(self.id()),
                index: 0,
            }),
        ))))
    }
    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let name = attr.as_str(vm.interns);
        if name == "release" {
            args.check_zero_args("memoryview.release", vm.heap)?;
            self.release(vm);
            return Ok(CallResult::Value(Value::None));
        }
        let mut guard = DropGuard::new(args, vm);
        let (_, vm) = guard.as_parts_mut();
        // Argument converters for cast and index precede the released-state check.
        if !matches!(name, "cast" | "index") {
            self.get(vm.heap).check_live()?;
        }
        let (args, vm) = guard.into_parts();
        let result = match name {
            "tobytes" => {
                let ToBytesArgs { order } = ToBytesArgs::from_args(args, vm)?;
                defer_drop!(order, vm);
                if !matches!(order.as_ref().map_or("C", |v| v.as_str(vm)), "C" | "F" | "A") {
                    return Err(ExcType::value_error("order must be 'C', 'F' or 'A'"));
                }
                let fortran = order.as_ref().is_some_and(|v| v.as_str(vm) == "F")
                    || (order.as_ref().is_some_and(|v| v.as_str(vm) == "A") && self.get(vm.heap).is_contiguous(true));
                super::bytes::allocate_bytes(self.get(vm.heap).to_vec_order(vm, fortran)?, vm.heap)
            }
            "count" | "index" => {
                let (value, start, stop) = if name == "count" {
                    (args.get_one_arg("count", vm.heap)?, None, None)
                } else {
                    let IndexArgs { value, start, stop } = IndexArgs::from_args(args, vm)?;
                    (value, start, stop)
                };
                defer_drop!(value, vm);
                defer_drop!(start, vm);
                defer_drop!(stop, vm);
                let len = i64::try_from(self.get(vm.heap).len).expect("view len");
                let start = start.as_ref().map_or(Ok(0), |v| search_bound(v, vm))?;
                let stop = stop.as_ref().map_or(Ok(len), |v| search_bound(v, vm))?;
                self.get(vm.heap).check_live()?;
                let ndim = self.get(vm.heap).ndim();
                if ndim == 0 {
                    return Err(ExcType::type_error(if name == "index" {
                        "invalid lookup on 0-dim memory"
                    } else {
                        "invalid indexing of 0-dim memory"
                    }));
                }
                if ndim > 1 {
                    return Err(not_implemented(if name == "index" {
                        "multi-dimensional lookup is not implemented"
                    } else {
                        "multi-dimensional sub-views are not implemented"
                    }));
                }
                let start = if start < 0 { start.saturating_add(len) } else { start }.clamp(0, len);
                let stop = if stop < 0 { stop.saturating_add(len) } else { stop }.clamp(0, len);
                let mut count = 0;
                for index in start..stop {
                    self.get(vm.heap).check_live()?;
                    let item = self
                        .get(vm.heap)
                        .item(usize::try_from(index).expect("view index"), vm)?;
                    defer_drop!(item, vm);
                    if item.py_eq_operator(value, vm)? {
                        if name == "index" {
                            return Ok(CallResult::Value(Value::Int(index)));
                        }
                        count += 1;
                    }
                    vm.heap
                        .tracker
                        .check_time_every(usize::try_from(index).expect("view index"))?;
                }
                if name == "index" {
                    return Err(ExcType::value_error("memoryview.index(x): x not found"));
                }
                Value::Int(count)
            }
            "tolist" => {
                args.check_zero_args("memoryview.tolist", vm.heap)?;
                let data = self.get(vm.heap);
                let mut cost = data
                    .len
                    .checked_mul(data.scalar_format().allocation_cost_hint())
                    .ok_or_else(ExcType::allocation_too_large)?;
                let mut containers = 1usize;
                for axis in 0..data.ndim() {
                    cost = cost
                        .checked_add(containers.checked_mul(128).ok_or_else(ExcType::allocation_too_large)?)
                        .ok_or_else(ExcType::allocation_too_large)?;
                    containers = containers
                        .checked_mul(data.dimension(axis))
                        .ok_or_else(ExcType::allocation_too_large)?;
                }
                check_repeat_size(cost, 1, &vm.heap.tracker)?;
                self.list_level(0, 0, vm)?
            }
            "toreadonly" => {
                args.check_zero_args("memoryview.toreadonly", vm.heap)?;
                let data = self.get(vm.heap);
                self.subview(data.start, data.step, data.len, true, data.format, vm)
            }
            "cast" => {
                let CastArgs { format, shape } = CastArgs::from_args(args, vm)?;
                defer_drop!(format, vm);
                defer_drop!(shape, vm);
                self.cast_view(format, shape.as_ref(), vm)?
            }
            "hex" => {
                let (sep, group) = super::bytes::parse_bytes_hex_args(args, vm)?;
                self.get(vm.heap).check_live()?;
                let bytes = self.get(vm.heap).to_vec(vm)?;
                super::bytes::hex_data(&bytes, sep, group, vm)?
            }
            "__enter__" => {
                args.check_zero_args("__enter__", vm.heap)?;
                return self.py_enter(vm);
            }
            "__exit__" => {
                args.drop_with(vm);
                return self.py_exit(vm, None);
            }
            _ => {
                args.drop_with(vm);
                return Err(ExcType::attribute_error(Type::MemoryView, name));
            }
        };
        Ok(CallResult::Value(result))
    }
}

#[derive(FromArgs)]
#[from_args(name = "tobytes")]
struct ToBytesArgs {
    #[from_args(default)]
    order: Option<StrArg>,
}
#[derive(FromArgs)]
#[from_args(name = "cast", bad_arg_named, at_most_total)]
struct CastArgs {
    format: StrArg,
    #[from_args(default)]
    shape: Option<Value>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct MemoryViewIterator {
    view: Value,
    index: usize,
}
impl HeapItem for MemoryViewIterator {
    fn py_dec_ref_ids(&mut self, ids: &mut Vec<HeapId>) {
        self.view.py_dec_ref_ids(ids);
    }
}
impl MemoryViewIterator {
    pub(crate) fn owner_id(&self) -> Option<HeapId> {
        if let Value::Ref(id) = self.view { Some(id) } else { None }
    }
}
impl<'h> PyTrait<'h> for HeapObjectRead<'h, MemoryViewIterator> {
    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }
    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::MemoryViewIterator
    }
    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }
    fn py_is_iterable(&self, _: &VM<'h>) -> bool {
        true
    }
    fn py_iter(&self, vm: &mut VM<'h>) -> RunResult<CallResult> {
        vm.heap.inc_ref(self.id());
        Ok(CallResult::Value(Value::Ref(self.id())))
    }
    fn py_next(&mut self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        let data = self.get(vm.heap);
        let Some(id) = data.owner_id() else {
            return Ok(None);
        };
        let view = vm.heap.read_as::<MemoryView>(id).expect("view iterator owner");
        if data.index >= view.get(vm.heap).len {
            drop(view);
            let owner = mem::replace(&mut self.get_mut(vm.heap).view, Value::None);
            owner.drop_with(vm);
            return Ok(None);
        }
        view.get(vm.heap).check_live()?;
        if view.get(vm.heap).ndim() > 1 {
            return Err(not_implemented("multi-dimensional sub-views are not implemented"));
        }
        let item = view.get(vm.heap).item(data.index, vm)?;
        self.get_mut(vm.heap).index += 1;
        Ok(Some(item))
    }
}

#[derive(FromArgs)]
#[from_args(name = "index", at_most_total)]
struct IndexArgs {
    #[from_args(pos_only)]
    value: Value,
    #[from_args(pos_only, default)]
    start: Option<Value>,
    #[from_args(pos_only, default)]
    stop: Option<Value>,
}

fn search_bound(value: &Value, vm: &mut VM<'_>) -> RunResult<i64> {
    let index = value
        .py_index_impl(vm)?
        .ok_or_else(|| ExcType::type_error("slice indices must be integers or have an __index__ method"))?;
    defer_drop!(index, vm);
    index.saturating_index(vm)
}

fn slice_length(start: i64, stop: i64, step: i64) -> usize {
    let count = if step > 0 {
        if start >= stop {
            0
        } else {
            (i128::from(stop) - i128::from(start) - 1) / i128::from(step) + 1
        }
    } else if start <= stop {
        0
    } else {
        (i128::from(start) - i128::from(stop) - 1) / -i128::from(step) + 1
    };
    usize::try_from(count).expect("normalized slice length")
}

fn invalid_key() -> RunError {
    ExcType::type_error("memoryview: invalid slice key")
}
fn not_implemented(message: &str) -> RunError {
    SimpleException::new_msg(ExcType::NotImplementedError, message).into()
}
fn structure_error() -> RunError {
    ExcType::value_error("memoryview assignment: lvalue and rvalue have different structures")
}
fn is_index_key(value: &Value, vm: &VM<'_>) -> bool {
    match value.py_type_heap(vm.heap) {
        Type::Int | Type::Bool => true,
        Type::Instance(class) => class_defines(class, "__index__", vm),
        _ => false,
    }
}
fn index_number(value: &Value, original: &Value, vm: &VM<'_>) -> RunResult<i64> {
    let number = match value.py_value(vm) {
        PyValue::Int(number) => number.to_i64(vm),
        PyValue::Bool(value) => Some(i64::from(value)),
        _ => None,
    };
    number.ok_or_else(|| {
        SimpleException::new_msg(
            ExcType::IndexError,
            format!("cannot fit '{}' into an index-sized integer", original.py_type_name(vm)),
        )
        .into()
    })
}
fn coerce_index(value: &Value, vm: &mut VM<'_>) -> RunResult<i64> {
    let Some(index) = value.py_index_impl(vm)? else {
        return Err(invalid_key());
    };
    defer_drop!(index, vm);
    index_number(index, value, vm)
}
fn normalize_index(index: i64, length: usize, axis: usize) -> RunResult<usize> {
    let length = i64::try_from(length).expect("dimension length");
    let index = if index < 0 { index.saturating_add(length) } else { index };
    if index < 0 || index >= length {
        return Err(SimpleException::new_msg(
            ExcType::IndexError,
            format!("index out of bounds on dimension {}", axis + 1),
        )
        .into());
    }
    Ok(usize::try_from(index).expect("normalized index"))
}

fn shape_items<'a>(shape: &Value, vm: &'a VM<'_>) -> RunResult<&'a [Value]> {
    if let Value::Ref(id) = shape {
        match vm.heap.get(*id) {
            HeapData::List(list) => return Ok(list.as_slice()),
            HeapData::Tuple(tuple) => return Ok(tuple.as_slice()),
            _ => {}
        }
    }
    Err(ExcType::type_error("shape must be a list or a tuple"))
}
