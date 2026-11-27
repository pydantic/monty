use std::mem::size_of;

use ahash::AHashMap;
use monty_types::ResourceTracker;
use num_bigint::BigInt;

use super::{legacy::Constructor, opcode as op};
use crate::{
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    identity::Identity,
    py_value::PyValue,
    types::{PyTrait, long_int::PyInt, py_trait::PyObjectIdentity},
    value::Value,
};

const MAX_DEPTH: usize = 128;

pub(super) fn encode(value: &Value, protocol: u8, vm: &VM<'_>) -> RunResult<Vec<u8>> {
    let mut encoder = Encoder {
        output: Vec::new(),
        memo: AHashMap::default(),
        tracker: &vm.heap.tracker,
        steps: 0,
        protocol,
    };
    encoder.write(&[op::PROTO, protocol])?;
    encoder.value(value, 0, vm)?;
    encoder.write(&[op::STOP])?;
    Ok(encoder.output)
}

struct Encoder<'a> {
    output: Vec<u8>,
    memo: AHashMap<Identity, u32>,
    tracker: &'a ResourceTracker,
    steps: usize,
    protocol: u8,
}

impl Encoder<'_> {
    fn value(&mut self, value: &Value, depth: usize, vm: &VM<'_>) -> RunResult<()> {
        if depth >= MAX_DEPTH {
            return Err(SimpleException::new_msg(ExcType::RecursionError, "pickle nesting limit exceeded").into());
        }
        self.tracker.check_memory_time_every(self.steps)?;
        self.steps += 1;
        match value {
            Value::Ref(_) | Value::InternString(_) | Value::InternBytes(_) | Value::InternLongInt(_) => {
                let identity = Identity::new(value);
                if let Some(&index) = self.memo.get(&identity) {
                    return self.reference(index);
                }
                self.data(value.py_value(vm), depth, vm)?;
                if self.memo.contains_key(&identity) {
                    Ok(())
                } else {
                    self.memoize(identity)
                }
            }
            _ => self.data(value.py_value(vm), depth, vm),
        }
    }

    fn data<'h>(&mut self, value: PyValue<'h>, depth: usize, vm: &VM<'h>) -> RunResult<()> {
        let child = depth + 1;
        match value {
            PyValue::None => self.write(&[op::NONE]),
            PyValue::Bool(true) => self.write(&[op::NEWTRUE]),
            PyValue::Bool(false) => self.write(&[op::NEWFALSE]),
            PyValue::Int(PyInt::Small(n)) => self.integer(n),
            PyValue::Int(PyInt::Interned(id)) => self.long(vm.interns.get_long_int(id)),
            PyValue::Int(PyInt::Heap(n)) => self.long(n.get(vm.heap).inner()),
            PyValue::Float(n) => {
                self.write(&[op::BINFLOAT])?;
                self.write(&n.to_be_bytes())
            }
            PyValue::Str(text) => {
                let bytes = text.as_str(vm).as_bytes();
                if self.protocol < 4 {
                    self.binary_header(op::BINUNICODE, bytes.len())?;
                    self.write(bytes)
                } else {
                    self.payload(bytes, [op::SHORT_BINUNICODE, op::BINUNICODE, op::BINUNICODE8])
                }
            }
            PyValue::Bytes(bytes) => self.bytes(bytes.as_slice(vm)),
            PyValue::List(list) => {
                self.write(&[op::EMPTY_LIST])?;
                self.memoize(list.py_identity())?;
                self.write(&[op::MARK])?;
                for item in list.get(vm.heap).items().map(|item| self.value(item, child, vm)) {
                    item?;
                }
                self.write(&[op::APPENDS])
            }
            PyValue::Tuple(tuple) => {
                self.write(&[op::MARK])?;
                for item in tuple.get(vm.heap).as_slice() {
                    self.value(item, child, vm)?;
                }
                // A child may have encoded this tuple through a mutable container.
                if let Some(&index) = self.memo.get(&tuple.py_identity()) {
                    self.write(&[op::POP_MARK])?;
                    self.reference(index)
                } else {
                    self.write(&[op::TUPLE])
                }
            }
            PyValue::Dict(dict) => {
                self.write(&[op::EMPTY_DICT])?;
                self.memoize(dict.py_identity())?;
                self.write(&[op::MARK])?;
                for (key, value) in dict.get(vm.heap) {
                    self.value(key, child, vm)?;
                    self.value(value, child, vm)?;
                }
                self.write(&[op::SETITEMS])
            }
            PyValue::Set(set) if self.protocol < 4 => {
                self.legacy_set(Constructor::Set, set.get(vm.heap).iter(), child, vm)
            }
            PyValue::FrozenSet(set) if self.protocol < 4 => {
                self.legacy_set(Constructor::FrozenSet, set.get(vm.heap).storage().iter(), child, vm)
            }
            PyValue::Set(set) => {
                self.write(&[op::EMPTY_SET])?;
                self.memoize(set.py_identity())?;
                self.write(&[op::MARK])?;
                for item in set.get(vm.heap).iter() {
                    self.value(item, child, vm)?;
                }
                self.write(&[op::ADDITEMS])
            }
            PyValue::FrozenSet(set) => {
                self.write(&[op::MARK])?;
                for item in set.get(vm.heap).storage().iter() {
                    self.value(item, child, vm)?;
                }
                self.write(&[op::FROZENSET])
            }
            _ => Err(ExcType::type_error(format_args!(
                "cannot pickle '{}' object",
                value.py_type_name(vm)
            ))),
        }
    }

    fn bytes(&mut self, bytes: &[u8]) -> RunResult<()> {
        if self.protocol != 2 {
            return self.payload(bytes, [op::SHORT_BINBYTES, op::BINBYTES, op::BINBYTES8]);
        }
        if bytes.is_empty() {
            self.global(Constructor::Bytes)?;
            return self.write(&[op::EMPTY_TUPLE, op::REDUCE]);
        }

        // Protocol 2 writes bytes as a Latin-1 string followed by a fixed encoding.
        let len = bytes
            .len()
            .saturating_add(bytes.iter().filter(|byte| !byte.is_ascii()).count());
        self.global(Constructor::Latin1)?;
        self.binary_header(op::BINUNICODE, len)?;
        self.reserve(len)?;
        for (index, &byte) in bytes.iter().enumerate() {
            let mut utf8 = [0; 2];
            self.output
                .extend_from_slice(char::from(byte).encode_utf8(&mut utf8).as_bytes());
            self.tracker.check_time_every(index)?;
        }
        self.binary_header(op::BINUNICODE, b"latin1".len())?;
        self.write(b"latin1")?;
        self.write(&[op::TUPLE2, op::REDUCE])
    }

    fn legacy_set<'a>(
        &mut self,
        constructor: Constructor,
        items: impl Iterator<Item = &'a Value>,
        depth: usize,
        vm: &VM<'_>,
    ) -> RunResult<()> {
        self.global(constructor)?;
        self.write(&[op::EMPTY_LIST, op::MARK])?;
        for item in items {
            self.value(item, depth, vm)?;
        }
        self.write(&[op::APPENDS, op::TUPLE1, op::REDUCE])
    }

    fn global(&mut self, constructor: Constructor) -> RunResult<()> {
        self.write(&[op::GLOBAL])?;
        self.write(constructor.name(self.protocol))
    }

    fn integer(&mut self, n: i64) -> RunResult<()> {
        if let Ok(n) = u8::try_from(n) {
            self.write(&[op::BININT1, n])
        } else if let Ok(n) = u16::try_from(n) {
            self.write(&[op::BININT2])?;
            self.write(&n.to_le_bytes())
        } else if let Ok(n) = i32::try_from(n) {
            self.write(&[op::BININT])?;
            self.write(&n.to_le_bytes())
        } else {
            self.long(&BigInt::from(n))
        }
    }

    fn long(&mut self, n: &BigInt) -> RunResult<()> {
        let size = usize::try_from(n.bits() / 8 + 1).map_err(|_| ExcType::allocation_too_large())?;
        self.tracker.check_allocation(size)?;
        let bytes = n.to_signed_bytes_le();
        if let Ok(len) = u8::try_from(bytes.len()) {
            self.write(&[op::LONG1, len])?;
        } else {
            let len = i32::try_from(bytes.len()).map_err(|_| ExcType::allocation_too_large())?;
            self.write(&[op::LONG4])?;
            self.write(&len.to_le_bytes())?;
        }
        self.write(&bytes)
    }

    fn payload(&mut self, bytes: &[u8], codes: [u8; 3]) -> RunResult<()> {
        if let Ok(len) = u8::try_from(bytes.len()) {
            self.write(&[codes[0], len])?;
        } else if u32::try_from(bytes.len()).is_ok() || self.protocol < 4 {
            self.binary_header(codes[1], bytes.len())?;
        } else {
            let len = u64::try_from(bytes.len()).map_err(|_| ExcType::allocation_too_large())?;
            self.write(&[codes[2]])?;
            self.write(&len.to_le_bytes())?;
        }
        self.write(bytes)
    }

    fn binary_header(&mut self, opcode: u8, len: usize) -> RunResult<()> {
        let len = u32::try_from(len).map_err(|_| ExcType::allocation_too_large())?;
        self.write(&[opcode])?;
        self.write(&len.to_le_bytes())
    }

    fn memoize(&mut self, identity: Identity) -> RunResult<()> {
        let len = self.memo.len();
        let index = u32::try_from(len).map_err(|_| ExcType::allocation_too_large())?;
        self.tracker
            .check_growth(len, self.memo.capacity(), size_of::<(Identity, u32)>())?;
        self.memo.try_reserve(1).map_err(|_| ExcType::allocation_too_large())?;
        self.memo.insert(identity, index);
        if self.protocol >= 4 {
            self.write(&[op::MEMOIZE])
        } else if let Ok(index) = u8::try_from(index) {
            self.write(&[op::BINPUT, index])
        } else {
            self.write(&[op::LONG_BINPUT])?;
            self.write(&index.to_le_bytes())
        }
    }

    fn reference(&mut self, index: u32) -> RunResult<()> {
        if let Ok(index) = u8::try_from(index) {
            self.write(&[op::BINGET, index])
        } else {
            self.write(&[op::LONG_BINGET])?;
            self.write(&index.to_le_bytes())
        }
    }

    fn write(&mut self, bytes: &[u8]) -> RunResult<()> {
        self.reserve(bytes.len())?;
        self.output.extend_from_slice(bytes);
        Ok(())
    }

    fn reserve(&mut self, additional: usize) -> RunResult<()> {
        let capacity = self.output.capacity();
        let required = self.output.len().saturating_add(additional);
        if required > capacity {
            let next_capacity = required.max(capacity.saturating_mul(2)).max(8);
            self.tracker.check_allocation(next_capacity - capacity)?;
            self.output
                .try_reserve_exact(next_capacity - self.output.len())
                .map_err(|_| ExcType::allocation_too_large())?;
        }
        Ok(())
    }
}
