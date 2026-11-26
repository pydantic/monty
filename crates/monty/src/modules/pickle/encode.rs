use std::mem::size_of;

use ahash::AHashMap;
use monty_types::ResourceTracker;
use num_bigint::BigInt;

use super::opcode as op;
use crate::{
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::HeapData,
    identity::Identity,
    value::Value,
};

const MAX_DEPTH: usize = 128;

pub(super) fn encode(value: &Value, protocol: u8, vm: &VM<'_>) -> RunResult<Vec<u8>> {
    let mut encoder = Encoder {
        output: Vec::new(),
        memo: AHashMap::default(),
        tracker: &vm.heap.tracker,
        steps: 0,
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
                self.data(value, depth, vm)?;
                if self.memo.contains_key(&identity) {
                    Ok(())
                } else {
                    self.memoize(identity)
                }
            }
            _ => self.data(value, depth, vm),
        }
    }

    fn data(&mut self, value: &Value, depth: usize, vm: &VM<'_>) -> RunResult<()> {
        if let Some(text) = value.as_str(vm) {
            return self.payload(text.as_bytes(), [op::SHORT_BINUNICODE, op::BINUNICODE, op::BINUNICODE8]);
        }
        if let Some(integer) = value.as_long_int(vm) {
            return self.long(integer);
        }
        let child = depth + 1;
        match value {
            Value::None => self.write(&[op::NONE]),
            Value::Bool(true) => self.write(&[op::NEWTRUE]),
            Value::Bool(false) => self.write(&[op::NEWFALSE]),
            Value::Int(n) => self.integer(*n),
            Value::Float(n) => {
                self.write(&[op::BINFLOAT])?;
                self.write(&n.to_be_bytes())
            }
            Value::InternBytes(id) => self.payload(
                vm.interns.get_bytes(*id),
                [op::SHORT_BINBYTES, op::BINBYTES, op::BINBYTES8],
            ),
            Value::Ref(id) => match vm.heap.get(*id) {
                HeapData::Bytes(bytes) => {
                    self.payload(bytes.as_slice(), [op::SHORT_BINBYTES, op::BINBYTES, op::BINBYTES8])
                }
                HeapData::List(list) => {
                    self.write(&[op::EMPTY_LIST])?;
                    self.memoize(Identity::from_heap_id(*id))?;
                    self.write(&[op::MARK])?;
                    for item in list.items().map(|item| self.value(item, child, vm)) {
                        item?;
                    }
                    self.write(&[op::APPENDS])
                }
                HeapData::Tuple(tuple) => {
                    self.write(&[op::MARK])?;
                    for item in tuple.as_slice() {
                        self.value(item, child, vm)?;
                    }
                    // A child may have encoded this tuple through a mutable container.
                    if let Some(&index) = self.memo.get(&Identity::from_heap_id(*id)) {
                        self.write(&[op::POP_MARK])?;
                        self.reference(index)
                    } else {
                        self.write(&[op::TUPLE])
                    }
                }
                HeapData::Dict(dict) => {
                    self.write(&[op::EMPTY_DICT])?;
                    self.memoize(Identity::from_heap_id(*id))?;
                    self.write(&[op::MARK])?;
                    for (key, value) in dict {
                        self.value(key, child, vm)?;
                        self.value(value, child, vm)?;
                    }
                    self.write(&[op::SETITEMS])
                }
                HeapData::Set(set) => {
                    self.write(&[op::EMPTY_SET])?;
                    self.memoize(Identity::from_heap_id(*id))?;
                    self.write(&[op::MARK])?;
                    for item in set.iter() {
                        self.value(item, child, vm)?;
                    }
                    self.write(&[op::ADDITEMS])
                }
                HeapData::FrozenSet(set) => {
                    self.write(&[op::MARK])?;
                    for item in set.storage().iter() {
                        self.value(item, child, vm)?;
                    }
                    self.write(&[op::FROZENSET])
                }
                _ => Err(unsupported(value, vm)),
            },
            _ => Err(unsupported(value, vm)),
        }
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
        } else if let Ok(len) = u32::try_from(bytes.len()) {
            self.write(&[codes[1]])?;
            self.write(&len.to_le_bytes())?;
        } else {
            let len = u64::try_from(bytes.len()).map_err(|_| ExcType::allocation_too_large())?;
            self.write(&[codes[2]])?;
            self.write(&len.to_le_bytes())?;
        }
        self.write(bytes)
    }

    fn memoize(&mut self, identity: Identity) -> RunResult<()> {
        let len = self.memo.len();
        let index = u32::try_from(len).map_err(|_| ExcType::allocation_too_large())?;
        self.tracker
            .check_growth(len, self.memo.capacity(), size_of::<(Identity, u32)>())?;
        self.memo.try_reserve(1).map_err(|_| ExcType::allocation_too_large())?;
        self.memo.insert(identity, index);
        self.write(&[op::MEMOIZE])
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
        let capacity = self.output.capacity();
        let required = self.output.len().saturating_add(bytes.len());
        if required > capacity {
            let next_capacity = required.max(capacity.saturating_mul(2)).max(8);
            self.tracker.check_allocation(next_capacity - capacity)?;
            self.output
                .try_reserve_exact(next_capacity - self.output.len())
                .map_err(|_| ExcType::allocation_too_large())?;
        }
        self.output.extend_from_slice(bytes);
        Ok(())
    }
}

fn unsupported(value: &Value, vm: &VM<'_>) -> RunError {
    ExcType::type_error(format_args!("cannot pickle '{}' object", value.py_type_name(vm)))
}
