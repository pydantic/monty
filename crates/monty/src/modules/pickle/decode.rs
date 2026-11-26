use std::str;

use num_bigint::BigInt;

use super::opcode as op;
use crate::{
    bytecode::VM,
    defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapId, HeapReadOutput},
    resource_checks::check_value_buffer_growth,
    types::{
        Dict, FrozenSet, List, LongInt, Set, TupleVec, allocate_tuple, bytes::allocate_bytes, str::allocate_string,
    },
    value::{VALUE_SIZE, Value},
};

pub(super) fn decode(input: &[u8], vm: &mut VM<'_>) -> RunResult<Value> {
    let Some([op::PROTO, 4 | 5, input @ ..]) = input.get(..) else {
        return Err(ExcType::value_error("pickle supports protocols 4 and 5"));
    };
    let decoder = Decoder {
        input,
        position: 0,
        frame_end: None,
        stack: Vec::new(),
        marks: Vec::new(),
        memo: Vec::new(),
    };
    defer_drop_mut!(decoder, vm);
    decoder.run(vm)
}

struct Decoder<'a> {
    input: &'a [u8],
    position: usize,
    frame_end: Option<usize>,
    stack: Vec<Value>,
    marks: Vec<usize>,
    memo: Vec<Value>,
}

impl<C: ContainsHeap> DropWithContext<C> for Decoder<'_> {
    fn drop_with(self, ctx: &mut C) {
        self.stack.drop_with(ctx);
        self.memo.drop_with(ctx);
    }
}

impl<'a> Decoder<'a> {
    fn run(&mut self, vm: &mut VM<'_>) -> RunResult<Value> {
        let mut steps = 0;
        loop {
            vm.heap.tracker.check_memory_time_every(steps)?;
            steps += 1;
            if self.frame_end == Some(self.position) {
                self.frame_end = None;
            }
            let opcode = self.byte()?;
            let value = match opcode {
                op::STOP => {
                    if self.stack.len() != 1 || !self.marks.is_empty() {
                        return Err(invalid_stack());
                    }
                    return self.pop();
                }
                op::FRAME => {
                    self.frame()?;
                    continue;
                }
                op::MARK => {
                    let (len, cap) = (self.marks.len(), self.marks.capacity());
                    vm.heap.tracker.check_growth(len, cap, size_of::<usize>())?;
                    self.marks.push(self.stack.len());
                    continue;
                }
                op::POP => {
                    if self.stack.len() == self.base() && !self.marks.is_empty() {
                        self.marks.pop();
                    } else {
                        self.pop()?.drop_with(vm);
                    }
                    continue;
                }
                op::POP_MARK => {
                    let start = self.mark()?;
                    self.stack.drain(start..).drop_with(vm);
                    continue;
                }
                op::MEMOIZE => {
                    let value = self.last()?;
                    let (len, cap) = (self.memo.len(), self.memo.capacity());
                    vm.heap.tracker.check_growth(len, cap, VALUE_SIZE)?;
                    self.memo.push(value.clone_with_heap(vm.heap));
                    continue;
                }
                op::BINGET | op::LONG_BINGET => {
                    let index = self.length(opcode == op::BINGET, false)?;
                    self.memo
                        .get(index)
                        .ok_or_else(|| ExcType::value_error("invalid pickle memo index"))?
                        .clone_with_heap(vm.heap)
                }
                op::NONE => Value::None,
                op::NEWTRUE => Value::Bool(true),
                op::NEWFALSE => Value::Bool(false),
                op::BININT1 => Value::Int(i64::from(self.byte()?)),
                op::BININT2 => Value::Int(i64::from(u16::from_le_bytes(self.array()?))),
                op::BININT => Value::Int(i64::from(i32::from_le_bytes(self.array()?))),
                op::LONG1 | op::LONG4 => {
                    let len = if opcode == op::LONG1 {
                        usize::from(self.byte()?)
                    } else {
                        usize::try_from(i32::from_le_bytes(self.array()?))
                            .map_err(|_| ExcType::value_error("invalid pickle integer length"))?
                    };
                    let bytes = self.read(len)?;
                    vm.heap.tracker.check_allocation(len.saturating_add(8))?;
                    LongInt::new(BigInt::from_signed_bytes_le(bytes)).into_value(vm.heap)
                }
                op::BINFLOAT => Value::Float(f64::from_be_bytes(self.array()?)),
                op::SHORT_BINUNICODE | op::BINUNICODE | op::BINUNICODE8 => {
                    let len = self.length(opcode == op::SHORT_BINUNICODE, opcode == op::BINUNICODE8)?;
                    let bytes = self.read(len)?;
                    let text =
                        str::from_utf8(bytes).map_err(|_| ExcType::value_error("invalid UTF-8 in pickle string"))?;
                    vm.heap.tracker.check_allocation(len)?;
                    allocate_string(text, vm.heap)
                }
                op::SHORT_BINBYTES | op::BINBYTES | op::BINBYTES8 => {
                    let len = self.length(opcode == op::SHORT_BINBYTES, opcode == op::BINBYTES8)?;
                    let bytes = self.read(len)?;
                    vm.heap.tracker.check_allocation(len)?;
                    allocate_bytes(bytes.to_vec(), vm.heap)
                }
                op::EMPTY_LIST => Value::allocate(List::new(Vec::new()), vm.heap),
                op::EMPTY_DICT => Value::allocate(Dict::new(), vm.heap),
                op::EMPTY_SET => Value::allocate(Set::new(), vm.heap),
                op::EMPTY_TUPLE | op::TUPLE1 | op::TUPLE2 | op::TUPLE3 | op::TUPLE => {
                    let start = match opcode {
                        op::EMPTY_TUPLE => self.stack.len(),
                        op::TUPLE => self.mark()?,
                        _ => self.start(usize::from(opcode - op::TUPLE1 + 1))?,
                    };
                    self.tuple(start, vm)?
                }
                op::APPEND | op::APPENDS => {
                    let start = if opcode == op::APPEND {
                        self.start(1)?
                    } else {
                        self.mark()?
                    };
                    self.append(start, vm)?;
                    continue;
                }
                op::SETITEM | op::SETITEMS => {
                    let start = if opcode == op::SETITEM {
                        self.start(2)?
                    } else {
                        self.mark()?
                    };
                    self.set_items(start, vm)?;
                    continue;
                }
                op::ADDITEMS => {
                    let start = self.mark()?;
                    self.add_items(start, vm)?;
                    continue;
                }
                op::FROZENSET => {
                    let start = self.mark()?;
                    self.frozenset(start, vm)?
                }
                _ => {
                    return Err(ExcType::value_error(format_args!(
                        "unsupported pickle opcode 0x{opcode:02x}"
                    )));
                }
            };
            self.push(value, vm)?;
        }
    }

    fn read(&mut self, len: usize) -> RunResult<&'a [u8]> {
        let remaining = &self.input[self.position..self.frame_end.unwrap_or(self.input.len())];
        let data = remaining
            .get(..len)
            .ok_or_else(|| ExcType::value_error("pickle data was truncated"))?;
        self.position += len;
        Ok(data)
    }

    fn array<const N: usize>(&mut self) -> RunResult<[u8; N]> {
        Ok(self.read(N)?.try_into().expect("read returns the requested length"))
    }

    fn byte(&mut self) -> RunResult<u8> {
        Ok(self.array::<1>()?[0])
    }

    fn length(&mut self, short: bool, wide: bool) -> RunResult<usize> {
        let size = if short {
            u64::from(self.byte()?)
        } else if wide {
            u64::from_le_bytes(self.array()?)
        } else {
            u64::from(u32::from_le_bytes(self.array()?))
        };
        usize::try_from(size).map_err(|_| ExcType::value_error("pickle length exceeds addressable size"))
    }

    fn frame(&mut self) -> RunResult<()> {
        if self.frame_end.is_some() {
            return Err(ExcType::value_error("nested pickle frame"));
        }
        let size = self.length(false, true)?;
        if size > self.input.len() - self.position {
            return Err(ExcType::value_error("pickle data was truncated"));
        }
        self.frame_end = Some(self.position + size);
        Ok(())
    }

    fn base(&self) -> usize {
        self.marks.last().copied().unwrap_or(0)
    }

    fn mark(&mut self) -> RunResult<usize> {
        self.marks.pop().ok_or_else(invalid_stack)
    }

    fn start(&self, count: usize) -> RunResult<usize> {
        self.stack
            .len()
            .checked_sub(count)
            .filter(|&start| start >= self.base())
            .ok_or_else(invalid_stack)
    }

    fn last(&self) -> RunResult<&Value> {
        Ok(&self.stack[self.start(1)?])
    }

    fn pop(&mut self) -> RunResult<Value> {
        self.start(1)?;
        self.stack.pop().ok_or_else(invalid_stack)
    }

    fn push(&mut self, value: Value, vm: &mut VM<'_>) -> RunResult<()> {
        let value = check_value_buffer_growth(vm, self.stack.len(), self.stack.capacity(), value)?;
        self.stack.push(value);
        Ok(())
    }

    fn tuple(&mut self, start: usize, vm: &VM<'_>) -> RunResult<Value> {
        let len = self.stack.len() - start;
        vm.heap.tracker.check_allocation(len.saturating_mul(VALUE_SIZE))?;
        let items = self.stack.drain(start..).collect::<TupleVec>();
        Ok(allocate_tuple(items, vm.heap))
    }

    fn target(&self, start: usize) -> RunResult<HeapId> {
        if start <= self.base() {
            return Err(invalid_stack());
        }
        self.stack[start - 1].ref_id().ok_or_else(invalid_stack)
    }

    fn append(&mut self, start: usize, vm: &mut VM<'_>) -> RunResult<()> {
        let HeapReadOutput::List(list) = vm.heap.read(self.target(start)?) else {
            return Err(invalid_stack());
        };
        let items = self.stack.drain(start..);
        defer_drop_mut!(items, vm);
        for (index, item) in items.enumerate() {
            list.append(vm, item)?;
            vm.heap.tracker.check_time_every(index)?;
        }
        Ok(())
    }

    fn set_items(&mut self, start: usize, vm: &mut VM<'_>) -> RunResult<()> {
        if !(self.stack.len() - start).is_multiple_of(2) {
            return Err(invalid_stack());
        }
        let HeapReadOutput::Dict(dict) = vm.heap.read(self.target(start)?) else {
            return Err(invalid_stack());
        };
        let items = self.stack.drain(start..);
        defer_drop_mut!(items, vm);
        let mut index = 0;
        while let Some(key) = items.next() {
            let value = items.next().expect("number of dictionary operands is even");
            let replaced = dict.set(key, value, vm)?;
            replaced.drop_with(vm);
            vm.heap.tracker.check_time_every(index)?;
            index += 1;
        }
        Ok(())
    }

    fn add_items(&mut self, start: usize, vm: &mut VM<'_>) -> RunResult<()> {
        let HeapReadOutput::Set(set) = vm.heap.read(self.target(start)?) else {
            return Err(invalid_stack());
        };
        let items = self.stack.drain(start..);
        defer_drop_mut!(items, vm);
        for (index, item) in items.enumerate() {
            set.add(item, vm)?;
            vm.heap.tracker.check_time_every(index)?;
        }
        Ok(())
    }

    fn frozenset(&mut self, start: usize, vm: &mut VM<'_>) -> RunResult<Value> {
        let items = self.stack.drain(start..);
        defer_drop_mut!(items, vm);
        let mut set = DropGuard::new(Set::new(), vm);
        let (values, vm) = set.as_parts_mut();
        for (index, item) in items.enumerate() {
            values.add(item, vm)?;
            vm.heap.tracker.check_time_every(index)?;
        }
        let (values, vm) = set.into_parts();
        Ok(Value::allocate(FrozenSet::from_set(values), vm.heap))
    }
}

fn invalid_stack() -> RunError {
    ExcType::value_error("invalid pickle stack")
}
