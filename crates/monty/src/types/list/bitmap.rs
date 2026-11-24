//! Bit storage for boolean list elements and for the positions of `None` in nullable lists.
//!
//! Each list index addresses one bit. Insertion and slice replacement can shift overlapping
//! ranges across word boundaries. The caller supplies the logical length because the last
//! allocated word can contain padding.

use crate::exception_private::{ExcType, ExcTypeExt, RunResult};

const BITS: usize = u64::BITS as usize;

#[derive(Debug, Default)]
pub(super) struct Bitmap(Vec<u64>);

impl Bitmap {
    pub(super) fn capacity(&self) -> usize {
        self.0.len().saturating_mul(BITS)
    }

    pub(super) fn allocation_bytes(slots: usize) -> usize {
        slots.div_ceil(BITS).saturating_mul(size_of::<u64>())
    }

    pub(super) fn growth_bytes(&self, slots: usize) -> usize {
        Self::allocation_bytes(slots).saturating_sub(self.0.capacity().saturating_mul(size_of::<u64>()))
    }

    pub(super) fn reserve(&mut self, slots: usize) -> RunResult<()> {
        let words = slots.div_ceil(BITS);
        if words > self.0.len() {
            self.0
                .try_reserve_exact(words - self.0.len())
                .map_err(|_| ExcType::allocation_too_large())?;
            self.0.resize(words, 0);
        }
        Ok(())
    }

    pub(super) fn get(&self, index: usize) -> bool {
        self.0[index / BITS] & (1 << (index % BITS)) != 0
    }

    pub(super) fn set(&mut self, index: usize, value: bool) {
        self.write(index, 1, u64::from(value));
    }

    pub(super) fn fill(&mut self, start: usize, count: usize, value: bool) {
        let mut offset = 0;
        while offset < count {
            let dst = start + offset;
            let width = (BITS - dst % BITS).min(count - offset);
            self.write(dst, width, if value { u64::MAX } else { 0 });
            offset += width;
        }
    }

    pub(super) fn swap(&mut self, a: usize, b: usize) {
        let first = self.get(a);
        self.set(a, self.get(b));
        self.set(b, first);
    }

    pub(super) fn reverse(&mut self, len: usize) {
        let words = &mut self.0[..len.div_ceil(BITS)];
        words.reverse();
        for word in &mut *words {
            *word = word.reverse_bits();
        }
        // Reversing whole words moves the last word's padding to the front.
        let tail = len % BITS;
        if tail != 0 {
            let padding = BITS - tail;
            for index in 0..words.len() - 1 {
                words[index] = (words[index] >> padding) | (words[index + 1] << tail);
            }
            *words.last_mut().expect("a partial word is present") >>= padding;
        }
    }

    pub(super) fn copy_from(&mut self, dst: usize, source: &Self, src: usize, count: usize) {
        let mut offset = 0;
        while offset < count {
            let start = dst + offset;
            let width = (BITS - start % BITS).min(count - offset);
            self.write(start, width, source.read(src + offset, width));
            offset += width;
        }
    }

    /// Moves an overlapping range without overwriting unread bits.
    pub(super) fn move_within(&mut self, src: usize, dst: usize, count: usize) {
        if src < dst {
            let mut end = count;
            while end != 0 {
                let width = ((dst + end - 1) % BITS + 1).min(end);
                end -= width;
                self.write(dst + end, width, self.read(src + end, width));
            }
        } else if src > dst {
            let mut offset = 0;
            while offset < count {
                let width = (BITS - (dst + offset) % BITS).min(count - offset);
                self.write(dst + offset, width, self.read(src + offset, width));
                offset += width;
            }
        }
    }

    fn read(&self, start: usize, width: usize) -> u64 {
        let shift = start % BITS;
        let mut bits = self.0[start / BITS] >> shift;
        if shift + width > BITS {
            bits |= self.0[start / BITS + 1] << (BITS - shift);
        }
        bits
    }

    fn write(&mut self, start: usize, width: usize, bits: u64) {
        let shift = start % BITS;
        let mask = (u64::MAX >> (BITS - width)) << shift;
        let word = &mut self.0[start / BITS];
        *word = (*word & !mask) | ((bits << shift) & mask);
    }
}
