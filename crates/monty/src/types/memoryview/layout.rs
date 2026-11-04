//! Physical layout and scalar access for direct byte-buffer views.

use super::{MemoryView, format::BufferFormat};
use crate::{
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    resource_checks::check_repeat_size,
    types::bytearray::read_buffer,
    value::Value,
};

impl MemoryView {
    pub(super) fn scalar_format(&self) -> BufferFormat {
        BufferFormat::from_code(self.format).expect("stored buffer format")
    }

    pub(crate) fn scalar_allocation_cost_hint(&self) -> usize {
        self.scalar_format().allocation_cost_hint()
    }

    pub(crate) fn format_name(&self) -> String {
        if self.native_prefix {
            format!("@{}", self.format)
        } else {
            self.format.to_string()
        }
    }

    pub(super) fn itemsize(&self) -> usize {
        self.scalar_format().itemsize()
    }
    pub(crate) fn ndim(&self) -> usize {
        self.shape.as_ref().map_or(1, Vec::len)
    }
    pub(super) fn dimension(&self, axis: usize) -> usize {
        self.shape.as_ref().map_or(self.len, |shape| shape[axis])
    }
    pub(super) fn stride(&self, axis: usize) -> i64 {
        if self.shape.is_none() {
            self.step
        } else {
            self.strides[axis]
        }
    }
    pub(super) fn shape_values(&self) -> Vec<usize> {
        self.shape.clone().unwrap_or_else(|| vec![self.len])
    }
    pub(super) fn stride_values(&self) -> Vec<i64> {
        if self.shape.is_none() {
            vec![self.step]
        } else {
            self.strides.clone()
        }
    }
    pub(super) fn nbytes(&self) -> usize {
        self.len.checked_mul(self.itemsize()).expect("view byte length")
    }

    pub(crate) fn checked_len(&self) -> RunResult<usize> {
        self.check_live()?;
        if self.ndim() == 0 {
            return Err(ExcType::type_error("0-dim memory has no length"));
        }
        Ok(self.dimension(0))
    }

    pub(super) fn is_contiguous(&self, fortran: bool) -> bool {
        // CPython requires the item stride for empty 1D views, but ignores singleton strides.
        if self.ndim() == 1 {
            return self.len == 1 || self.stride(0) == i64::try_from(self.itemsize()).expect("item size");
        }
        if self.len == 0 {
            return true;
        }
        let mut expected = i64::try_from(self.itemsize()).expect("item size");
        for index in 0..self.ndim() {
            let axis = if fortran { index } else { self.ndim() - index - 1 };
            let length = self.dimension(axis);
            if length > 1 && self.stride(axis) != expected {
                return false;
            }
            expected = expected
                .checked_mul(i64::try_from(length).expect("dimension"))
                .expect("view extent");
        }
        true
    }

    pub(crate) fn contiguous<'a>(&self, vm: &'a VM<'_>) -> RunResult<&'a [u8]> {
        self.check_live()?;
        if !self.is_contiguous(false) {
            return Err(SimpleException::new_msg(
                ExcType::BufferError,
                "memoryview: underlying buffer is not C-contiguous",
            )
            .into());
        }
        let source = read_buffer(&self.owner, vm).expect("memoryview owns byte storage");
        let start = if self.len == 0 {
            0
        } else {
            usize::try_from(self.start).expect("view offset")
        };
        Ok(&source[start..start + self.nbytes()])
    }

    pub(crate) fn to_vec(&self, vm: &VM<'_>) -> RunResult<Vec<u8>> {
        self.to_vec_order(vm, false)
    }

    pub(super) fn to_vec_order(&self, vm: &VM<'_>, fortran: bool) -> RunResult<Vec<u8>> {
        self.check_live()?;
        check_repeat_size(self.nbytes(), 1, &vm.heap.tracker)?;
        if self.is_contiguous(false) && (!fortran || self.is_contiguous(true)) {
            return Ok(self.contiguous(vm)?.to_vec());
        }
        let source = read_buffer(&self.owner, vm).expect("view owner");
        let mut result = Vec::with_capacity(self.nbytes());
        for index in 0..self.len {
            let offset = self.offset_order(index, fortran);
            result.extend_from_slice(&source[offset..offset + self.itemsize()]);
            vm.heap.tracker.check_time_every(index)?;
        }
        Ok(result)
    }

    pub(super) fn offset(&self, index: usize) -> usize {
        self.offset_order(index, false)
    }

    pub(super) fn offset_order(&self, mut index: usize, fortran: bool) -> usize {
        let mut offset = i128::from(self.start);
        for dimension in 0..self.ndim() {
            let axis = if fortran {
                dimension
            } else {
                self.ndim() - dimension - 1
            };
            let length = self.dimension(axis);
            let coordinate = index % length;
            index /= length;
            offset += i128::try_from(coordinate).expect("coordinate") * i128::from(self.stride(axis));
        }
        usize::try_from(offset).expect("normalized view offset")
    }

    pub(super) fn item(&self, index: usize, vm: &VM<'_>) -> RunResult<Value> {
        let source = read_buffer(&self.owner, vm).expect("view owner");
        let start = self.offset(index);
        self.scalar_format().unpack(&source[start..start + self.itemsize()], vm)
    }
}

#[cfg(test)]
mod tests {
    use crate::{hash::HashValue, types::MemoryView, value::Value};

    #[derive(serde::Serialize)]
    struct LegacyView {
        owner: Value,
        start: i64,
        step: i64,
        len: usize,
        readonly: bool,
        format: char,
        native_prefix: bool,
        cached_hash: Option<HashValue>,
    }

    #[test]
    fn legacy_byte_view_layout_defaults() {
        let old = LegacyView {
            owner: Value::None,
            start: 2,
            step: -1,
            len: 3,
            readonly: true,
            format: 'B',
            native_prefix: false,
            cached_hash: None,
        };
        let bytes = minicbor_serde::to_vec(&old).unwrap();
        let restored: MemoryView = minicbor_serde::from_slice(&bytes).unwrap();
        assert_eq!(restored.shape_values(), vec![3]);
        assert_eq!(restored.stride_values(), vec![-1]);
        assert_eq!(restored.ndim(), 1);
        assert_eq!(restored.itemsize(), 1);
        assert!(restored.check_live().is_err());
    }
}
