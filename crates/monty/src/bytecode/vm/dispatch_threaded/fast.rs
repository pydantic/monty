//! Fast opcode handlers and their shared iterator fallback.

#![expect(
    clippy::inline_always,
    reason = "Fast handler bodies must inline into scalar entry points."
)]

use std::{cmp::Ordering, mem};

use super::{
    super::{CallResult, HeapId, HeapReadOutput, Opcode, PostConversionEffect, PyTrait, PyValue, VM, Value},
    Dispatch, DispatchContext, Status,
};
use crate::bytecode::subscription::{GlobalSubscript, GlobalSubscriptKey, LocalSubscript, LocalSubscriptKey};

#[inline(always)]
pub(super) fn native_nop(d: Dispatch<'_, '_>) -> Status {
    d.next()
}

#[inline(always)]
pub(super) fn native_literal<const OP: u8>(mut d: Dispatch<'_, '_>) -> Status {
    if d.vm.stack.len() == d.vm.stack.capacity() {
        return d.fallback(OP);
    }

    match OP {
        byte if byte == Opcode::LoadNone as u8 => d.vm.push(Value::None),
        byte if byte == Opcode::LoadTrue as u8 => d.vm.push(Value::Bool(true)),
        byte if byte == Opcode::LoadFalse as u8 => d.vm.push(Value::Bool(false)),
        byte if byte == Opcode::LoadSmallInt as u8 => {
            let value = d.read_u8().cast_signed();
            d.vm.push(Value::Int(i64::from(value)));
        }
        _ => unreachable!(),
    }
    d.next()
}

#[inline(always)]
pub(super) fn native_constant(mut d: Dispatch<'_, '_>) -> Status {
    if d.vm.stack.len() == d.vm.stack.capacity() {
        return d.fallback(Opcode::LoadConst as u8);
    }

    let slot = d.read_u16();
    // Compiled constants hold only literals and interned IDs.
    let constant = d.vm.current_frame.code.constant(slot);
    if let Value::InternString(value) = constant {
        d.vm.push(Value::InternString(*value));
        return d.next();
    }
    if let Value::Float(value) = constant {
        d.vm.push(Value::Float(*value));
        return d.next();
    }
    if let Value::Int(value) = constant {
        d.vm.push(Value::Int(*value));
        return d.next();
    }
    let value = constant.copy_immediate();
    d.vm.push(value);

    d.next()
}

#[inline(always)]
pub(super) fn native_local<const OP: u8>(mut d: Dispatch<'_, '_>) -> Status {
    if d.vm.stack.len() == d.vm.stack.capacity() {
        return d.fallback(OP);
    }

    let base = d.vm.current_frame.stack_base();
    let slot = if OP == Opcode::LoadLocal0 as u8 {
        0
    } else if OP == Opcode::LoadLocal1 as u8 {
        1
    } else if OP == Opcode::LoadLocal2 as u8 {
        2
    } else if OP == Opcode::LoadLocal3 as u8 {
        3
    } else if OP == Opcode::LoadLocal as u8 {
        usize::from(d.read_u8())
    } else {
        usize::from(d.read_u16())
    };
    if matches!(d.vm.stack[base + slot], Value::Undefined) {
        return d.fallback(OP);
    }
    if let Value::Int(value) = d.vm.stack[base + slot] {
        d.vm.push(Value::Int(value));
        return d.next();
    }
    if let Value::Float(value) = d.vm.stack[base + slot] {
        d.vm.push(Value::Float(value));
        return d.next();
    }
    if let Value::Ref(id) = d.vm.stack[base + slot] {
        // Reserve before the refcount write so LLVM can exclude stack allocation.
        // Incrementing a reference cannot execute Python or allocate; the temporary None is unobservable.
        d.vm.push(Value::None);
        d.vm.heap.inc_ref(id);
        *d.vm.stack.last_mut().expect("reserved operand") = Value::Ref(id);
    } else {
        let value = d.vm.stack[base + slot].copy_immediate();
        d.vm.push(value);
    }

    d.next()
}

#[inline(always)]
pub(super) fn native_store<const OP: u8>(mut d: Dispatch<'_, '_>) -> Status {
    let base = d.vm.current_frame.stack_base();
    let slot = if OP == Opcode::StoreLocal as u8 {
        usize::from(d.read_u8())
    } else {
        usize::from(d.read_u16())
    };
    // Updating numeric payloads avoids copying padding through the operand stack.
    if let Value::Int(value) = *d.vm.peek()
        && let Value::Int(local) = &mut d.vm.stack[base + slot]
    {
        *local = value;
        d.vm.pop();
        return d.next();
    }
    if let Value::Float(value) = *d.vm.peek()
        && let Value::Float(local) = &mut d.vm.stack[base + slot]
    {
        *local = value;
        d.vm.pop();
        return d.next();
    }
    // Replacing an immediate local cannot release a heap object.
    if matches!(d.vm.stack[base + slot], Value::Ref(_)) {
        return d.fallback(OP);
    }
    let value = d.vm.pop();
    d.vm.stack[base + slot] = value;

    d.next()
}

#[inline(always)]
pub(super) fn native_stack<const OP: u8>(mut d: Dispatch<'_, '_>) -> Status {
    let growth = if OP == Opcode::Dup2 as u8 {
        2
    } else {
        usize::from(OP == Opcode::Dup as u8)
    };
    if d.vm.stack.capacity() - d.vm.stack.len() < growth {
        return d.fallback(OP);
    }

    match OP {
        byte if byte == Opcode::Pop as u8 => {
            if matches!(d.vm.peek(), Value::Ref(_)) {
                return d.fallback(OP);
            }
            d.vm.pop();
        }
        byte if byte == Opcode::Dup as u8 => {
            if let Value::Ref(id) = *d.vm.peek() {
                d.vm.push(Value::None);
                d.vm.heap.inc_ref(id);
                *d.vm.stack.last_mut().expect("reserved operand") = Value::Ref(id);
            } else {
                let value = d.vm.peek().copy_immediate();
                d.vm.push(value);
            }
        }
        byte if byte == Opcode::Rot2 as u8 => {
            let len = d.vm.stack.len();
            d.vm.stack.swap(len - 1, len - 2);
        }
        byte if byte == Opcode::Dup2 as u8 => {
            let len = d.vm.stack.len();
            if matches!(d.vm.stack[len - 2], Value::Ref(_)) || matches!(d.vm.stack[len - 1], Value::Ref(_)) {
                d.vm.push(Value::None);
                d.vm.push(Value::None);
                let first = d.vm.stack[len - 2].clone_with_heap(d.vm.heap);
                let second = d.vm.stack[len - 1].clone_with_heap(d.vm.heap);
                d.vm.stack[len] = first;
                d.vm.stack[len + 1] = second;
            } else {
                let first = d.vm.stack[len - 2].copy_immediate();
                let second = d.vm.stack[len - 1].copy_immediate();
                d.vm.push(first);
                d.vm.push(second);
            }
        }
        byte if byte == Opcode::Rot3 as u8 => {
            let len = d.vm.stack.len();
            d.vm.stack[len - 3..].rotate_right(1);
        }
        byte if byte == Opcode::LiftToTop as u8 => {
            let count = usize::from(d.read_u8());
            let start = d.vm.stack.len() - 1 - count;
            d.vm.stack[start..].rotate_left(1);
        }
        _ => unreachable!(),
    }
    d.next()
}

#[inline(always)]
pub(super) fn native_branch<const OP: u8>(mut d: Dispatch<'_, '_>) -> Status {
    let taken = if OP == Opcode::Jump as u8 {
        true
    } else {
        // Immediate truth values cannot execute Python; other operands use the semantic handler.
        let truth = match d.vm.peek() {
            Value::Bool(value) => *value,
            Value::Int(value) => *value != 0,
            Value::None => false,
            _ => return d.fallback(OP),
        };
        let taken = truth == (OP == Opcode::JumpIfTrue as u8 || OP == Opcode::JumpIfTrueOrPop as u8);
        if !taken || (OP != Opcode::JumpIfTrueOrPop as u8 && OP != Opcode::JumpIfFalseOrPop as u8) {
            // The truth test above admits only immediate values.
            d.vm.pop();
        }
        taken
    };
    let offset = d.read_u16().cast_signed();
    if taken {
        jump_relative!(d.ip, offset);
    }
    d.next()
}

#[inline(always)]
pub(super) fn numeric<const OP: u8>(d: Dispatch<'_, '_>) -> Status {
    let handled = if OP == Opcode::BinaryAdd as u8 || OP == Opcode::InplaceAdd as u8 {
        d.vm.binary_numeric_immediate(i64::checked_add, |lhs, rhs| lhs + rhs)
    } else if OP == Opcode::BinarySub as u8 || OP == Opcode::InplaceSub as u8 {
        d.vm.binary_numeric_immediate(i64::checked_sub, |lhs, rhs| lhs - rhs)
    } else {
        d.vm.binary_numeric_immediate(i64::checked_mul, |lhs, rhs| lhs * rhs)
    };
    if !handled {
        return d.fallback(OP);
    }
    d.next()
}

#[inline(always)]
pub(super) fn for_iter(mut d: Dispatch<'_, '_>) -> Status {
    if d.vm.stack.len() == d.vm.stack.capacity() {
        return d.fallback(Opcode::ForIter as u8);
    }
    let Value::Ref(heap_id) = *d.vm.peek() else {
        return d.fallback(Opcode::ForIter as u8);
    };
    let result = match d.vm.heap.read(heap_id) {
        // These concrete iterators cannot execute Python or suspend.
        HeapReadOutput::RangeIterator(mut iter) => iter.next_item(d.vm.heap),
        HeapReadOutput::ListIterator(mut iter) => iter.next_item(d.vm.heap),
        HeapReadOutput::TupleIterator(mut iter) => iter.next_item(d.vm.heap),
        iter => {
            let offset = i16::from_le_bytes([d.bytes[d.ip], d.bytes[d.ip + 1]]);
            d.vm.current_frame.ip = d.ip + 2;
            d.vm.instruction_ip = d.start;
            let status = advance_iterator(d.vm, d.context, iter, heap_id, offset);
            if status != Status::Continue {
                return status;
            }
            return d.resume();
        }
    };
    if let Some(value) = result {
        // The jump offset is needed only when the iterator is exhausted.
        d.ip += 2;
        // A for-loop usually stores its next item immediately. Keep both instruction
        // counts, and leave heap replacement and intervening checkpoints to the handlers.
        if d.remaining != 0 {
            let opcode = d.bytes[d.ip];
            let operand = if opcode == Opcode::StoreLocal as u8 {
                1
            } else if opcode == Opcode::StoreLocalW as u8 {
                2
            } else {
                0
            };
            if operand != 0 {
                let slot = if operand == 1 {
                    usize::from(d.bytes[d.ip + 1])
                } else {
                    usize::from(u16::from_le_bytes([d.bytes[d.ip + 1], d.bytes[d.ip + 2]]))
                };
                let local = &mut d.vm.stack[d.vm.current_frame.stack_base() + slot];
                if !matches!(local, Value::Ref(_)) {
                    *local = value;
                    d.start = d.ip;
                    d.ip += operand + 1;
                    d.remaining -= 1;
                    return d.next();
                }
            }
        }
        d.vm.push(value);
        d.next()
    } else {
        d.vm.pop().drop_with(d.vm);
        let offset = d.read_u16().cast_signed();
        jump_relative!(d.ip, offset);
        d.next()
    }
}

#[inline(always)]
pub(super) fn comparison<const OP: u8>(d: Dispatch<'_, '_>) -> Status {
    let handled = d.vm.compare_numeric(|ordering| match OP {
        byte if byte == Opcode::CompareEq as u8 => ordering == Some(Ordering::Equal),
        byte if byte == Opcode::CompareNe as u8 => ordering != Some(Ordering::Equal),
        byte if byte == Opcode::CompareLt as u8 => ordering.is_some_and(Ordering::is_lt),
        byte if byte == Opcode::CompareLe as u8 => ordering.is_some_and(Ordering::is_le),
        byte if byte == Opcode::CompareGt as u8 => ordering.is_some_and(Ordering::is_gt),
        byte if byte == Opcode::CompareGe as u8 => ordering.is_some_and(Ordering::is_ge),
        _ => unreachable!(),
    });
    if !handled {
        return d.fallback(OP);
    }
    d.next()
}

#[inline(never)]
pub(super) fn advance_iterator<'h>(
    vm: &mut VM<'h>,
    context: &mut DispatchContext,
    iter: HeapReadOutput<'h>,
    heap_id: HeapId,
    offset: i16,
) -> Status {
    // A file nobody has read yet has no buffer to slice: load it from
    // the host, then re-dispatch this instruction against the buffer.
    if let HeapReadOutput::OpenFile(file) = &iter
        && file.needs_buffer_load(vm)
    {
        let effect = PostConversionEffect::BufferLoad { file_id: heap_id };
        let call = file.buffer_load_call(vm, effect);
        drop(iter);
        vm.current_frame.ip = vm.instruction_ip;
        handle_call_result!(vm, context, Ok(call));
        return Status::Continue;
    }

    let mut iter = PyValue::from(iter);
    match iter.py_next(vm) {
        Ok(Some(value)) => vm.push(value),
        Ok(None) => {
            // Release the iterator view before decrementing its owned reference.
            drop(iter);
            // Iterator exhausted - pop it and jump to end
            let iter = vm.pop();
            iter.drop_with(vm);
            jump_relative!(vm.current_frame.ip, offset);
        }
        Err(e) => {
            // Drop the HeapRead before dec_ref to release the reader count
            drop(iter);
            // Error during iteration (e.g., dict size changed)
            let iter = vm.pop();
            iter.drop_with(vm);
            catch!(vm, context, e);
        }
    }
    Status::Continue
}

#[inline(always)]
pub(super) fn division<const OP: u8>(d: Dispatch<'_, '_>) -> Status {
    let handled = if OP == Opcode::BinaryDiv as u8 || OP == Opcode::InplaceDiv as u8 {
        d.vm.binary_div_immediate()
    } else if OP == Opcode::BinaryFloorDiv as u8 || OP == Opcode::InplaceFloorDiv as u8 {
        d.vm.binary_floordiv_immediate()
    } else {
        d.vm.binary_mod_immediate()
    };
    if !handled {
        return d.fallback(OP);
    }
    d.next()
}

#[inline(always)]
pub(super) fn bitwise<const OP: u8>(d: Dispatch<'_, '_>) -> Status {
    let len = d.vm.stack.len();
    if let (Value::Int(lhs), Value::Int(rhs)) = (&d.vm.stack[len - 2], &d.vm.stack[len - 1]) {
        let result = if OP == Opcode::BinaryAnd as u8 || OP == Opcode::InplaceAnd as u8 {
            Some(lhs & rhs)
        } else if OP == Opcode::BinaryOr as u8 || OP == Opcode::InplaceOr as u8 {
            Some(lhs | rhs)
        } else if OP == Opcode::BinaryXor as u8 || OP == Opcode::InplaceXor as u8 {
            Some(lhs ^ rhs)
        } else if OP == Opcode::BinaryRShift as u8 || OP == Opcode::InplaceRShift as u8 {
            if *rhs < 0 {
                None
            } else if *rhs >= 64 {
                Some(if *lhs < 0 { -1 } else { 0 })
            } else {
                Some(lhs >> rhs)
            }
        } else if (0..64).contains(rhs) {
            // checked_shl checks only the count, so check the widened result as well.
            i64::try_from(i128::from(*lhs) << rhs).ok()
        } else {
            None
        };
        if let Some(result) = result {
            d.vm.stack[len - 2] = Value::Int(result);
            d.vm.stack.truncate(len - 1);
            return d.next();
        }
    }
    d.fallback(OP)
}

#[inline(always)]
pub(super) fn unary<const OP: u8>(d: Dispatch<'_, '_>) -> Status {
    let result = match d.vm.peek() {
        Value::Int(value) => {
            if OP == Opcode::UnaryNeg as u8 {
                value.checked_neg().map(Value::Int)
            } else if OP == Opcode::UnaryInvert as u8 {
                Some(Value::Int(!value))
            } else if OP == Opcode::UnaryNot as u8 {
                Some(Value::Bool(*value == 0))
            } else {
                Some(Value::Int(*value))
            }
        }
        Value::Float(value) if OP != Opcode::UnaryInvert as u8 => {
            if OP == Opcode::UnaryNeg as u8 {
                Some(Value::Float(-value))
            } else if OP == Opcode::UnaryNot as u8 {
                Some(Value::Bool(*value == 0.0))
            } else {
                Some(Value::Float(*value))
            }
        }
        Value::Bool(value) if OP == Opcode::UnaryNot as u8 => Some(Value::Bool(!value)),
        Value::None if OP == Opcode::UnaryNot as u8 => Some(Value::Bool(true)),
        _ => None,
    };
    if let Some(result) = result {
        let len = d.vm.stack.len();
        d.vm.stack[len - 1] = result;
        d.next()
    } else {
        d.fallback(OP)
    }
}

#[inline(always)]
pub(super) fn native_subscript_store(mut d: Dispatch<'_, '_>) -> Status {
    let object = d.read_u8();
    let key = d.read_u8();
    let destination = d.read_u8();
    if d.vm.try_subscript_to_local(object, key, destination) {
        return d.next();
    }
    d.fallback(Opcode::SubscrToLocal as u8)
}

#[inline(always)]
pub(super) fn native_subscript<const OP: u8>(mut d: Dispatch<'_, '_>) -> Status {
    let base = d.vm.current_frame.stack_base();
    let value = if OP == Opcode::LoadSubscrLocal as u8 {
        let object = usize::from(d.read_u8());
        let key = usize::from(d.read_u8());
        d.vm.native_subscript_view(&d.vm.stack[base + object], &d.vm.stack[base + key])
    } else {
        let key = d.read_u16();
        let object_kind = d.read_u8();
        let operand = LocalSubscript::decode_constant(key, object_kind);
        let object = &d.vm.stack[base + usize::from(operand.object())];
        match operand.key() {
            LocalSubscriptKey::String(key) => d.vm.native_const_subscript_view(object, key, true),
            LocalSubscriptKey::Integer(key) => d.vm.native_const_subscript_view(object, key.cast_unsigned(), false),
            LocalSubscriptKey::Local(_) => unreachable!("constant subscription has a literal key"),
        }
    };
    let Some(value) = value else {
        return d.fallback(OP);
    };
    (d.ip, d.remaining) = if let Value::Int(number) = value {
        let number = *number;
        publish_subscript_int(d.vm, d.bytes, d.ip, d.remaining, number)
    } else {
        let value = value.clone_with_heap(d.vm.heap);
        d.vm.push(value);
        (d.ip, d.remaining)
    };
    d.next()
}

#[inline(always)]
pub(super) fn native_global_subscript(mut d: Dispatch<'_, '_>) -> Status {
    if d.remaining < 2 {
        return d.fallback(Opcode::LoadGlobalSubscr as u8);
    }
    let operand = GlobalSubscript::decode(d.read_u16());
    let key_ip = d.ip;
    let base = d.vm.current_frame.stack_base();
    let word = || u16::from_le_bytes([d.bytes[key_ip + 1], d.bytes[key_ip + 2]]);
    let object = &d.vm.globals[usize::from(operand.slot())];
    let (value, width) = match operand.key() {
        Some(GlobalSubscriptKey::Global) => (
            d.vm.native_subscript_view(object, &d.vm.globals[usize::from(word())]),
            3,
        ),
        Some(GlobalSubscriptKey::Constant) => (
            d.vm.native_subscript_view(object, d.vm.current_frame.code.constant(word())),
            3,
        ),
        Some(GlobalSubscriptKey::SmallInteger) => {
            let key = i16::from(d.bytes[key_ip + 1].cast_signed()).cast_unsigned();
            (d.vm.native_const_subscript_view(object, key, false), 2)
        }
        Some(GlobalSubscriptKey::FixedLocal) => (
            d.vm.native_subscript_view(
                object,
                &d.vm.stack[base + usize::from(d.bytes[key_ip] - Opcode::LoadLocal0 as u8)],
            ),
            1,
        ),
        Some(GlobalSubscriptKey::Local) => (
            d.vm.native_subscript_view(object, &d.vm.stack[base + usize::from(d.bytes[key_ip + 1])]),
            2,
        ),
        Some(GlobalSubscriptKey::WideLocal) => (
            d.vm.native_subscript_view(object, &d.vm.stack[base + usize::from(word())]),
            3,
        ),
        _ => return d.fallback(Opcode::LoadGlobalSubscr as u8),
    };
    debug_assert_eq!(d.bytes[key_ip + width], Opcode::BinarySubscr as u8);
    let Some(value) = value else {
        return d.fallback(Opcode::LoadGlobalSubscr as u8);
    };

    // The original key load and subscript remain in the bytecode for fallback.
    let target = key_ip + width + 1;
    d.remaining -= 2;
    (d.ip, d.remaining) = if let Value::Int(number) = value {
        let number = *number;
        publish_subscript_int(d.vm, d.bytes, target, d.remaining, number)
    } else {
        let value = value.clone_with_heap(d.vm.heap);
        d.vm.push(value);
        (target, d.remaining)
    };
    d.next()
}

/// Folds an integer store using dispatch-local offsets; a checkpoint keeps the store pending.
#[inline(always)]
#[expect(
    clippy::inline_always,
    reason = "keep result publication and continuation in each handler"
)]
fn publish_subscript_int(vm: &mut VM<'_>, bytes: &[u8], ip: usize, remaining: u32, number: i64) -> (usize, u32) {
    if remaining == 0 {
        vm.push(Value::Int(number));
        return (ip, remaining);
    }

    let (destination, width) = match bytes.get(ip).copied() {
        Some(op) if op == Opcode::StoreLocal as u8 => {
            let slot = usize::from(bytes[ip + 1]);
            (&mut vm.stack[vm.current_frame.stack_base() + slot], 2)
        }
        Some(op) if op == Opcode::StoreLocalW as u8 => {
            let slot = usize::from(u16::from_le_bytes([bytes[ip + 1], bytes[ip + 2]]));
            (&mut vm.stack[vm.current_frame.stack_base() + slot], 3)
        }
        Some(op) if op == Opcode::StoreGlobal as u8 => {
            let slot = usize::from(u16::from_le_bytes([bytes[ip + 1], bytes[ip + 2]]));
            (&mut vm.globals[slot], 3)
        }
        _ => {
            vm.push(Value::Int(number));
            return (ip, remaining);
        }
    };
    if let Value::Int(previous) = destination {
        *previous = number;
    } else {
        let old = mem::replace(destination, Value::Int(number));
        old.drop_with(vm);
    }
    (ip + width, remaining - 1)
}
