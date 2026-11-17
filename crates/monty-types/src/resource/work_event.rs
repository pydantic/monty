//! The kinds of work the work clock charges for, beyond one instruction's own weight.

use strum::{EnumCount, EnumIter, IntoStaticStr};

use super::work_weights::EVENT_MILLI_UNITS;

/// One kind of work charged through [`ResourceTracker::charge`](super::ResourceTracker::charge).
///
/// A charge names what happened and how often; the weight of one occurrence is fitted, not
/// written at the call site. `make fit-work-clock` measures every event's count per script and
/// regenerates the weights, so a new kind of work needs a variant here and a charge where it
/// happens, nothing more. Variants are appended: the generated table is in declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumCount, EnumIter, IntoStaticStr)]
#[repr(u8)]
pub enum WorkEvent {
    /// One bytecode instruction, before its opcode's own weight.
    Instruction,
    /// One un-amortized limit poll by a native operation.
    Poll,
    /// One item of a native loop that polls the limits.
    LoopItem,
    /// Creating an iterator.
    IterNew,
    /// Taking one item from an iterator.
    IterNext,
    /// A binary operator applied to anything but two small numbers.
    SlowBinary,
    /// A comparison of operands that are not both small numbers.
    SlowCompare,
    /// An `assert` comparing operands that are not both small numbers.
    AssertCompare,
    /// Calling a heap callable: a closure, a function with defaults, a class, a bound method.
    HeapCall,
    /// Calling a method on an instance of a Python class.
    MethodCall,
    /// Allocating a bound method.
    BoundMethod,
    /// Resolving a builtin by name at run time.
    BuiltinLookup,
    /// One item hashed into a dict or set display.
    HashedItem,
    /// One part joined into an f-string.
    FStringPart,
    /// One limb multiplication in big-integer arithmetic.
    BigintProduct,
    /// One limb of a big integer written in decimal.
    DecimalLimb,
    /// One byte of JSON parsed.
    JsonLoadByte,
    /// One byte of JSON written.
    JsonDumpByte,
    /// One byte of subject a regex scans.
    RegexScanByte,
    /// One regex match found.
    RegexMatch,
    /// One byte a substring search or replacement scans.
    StrScanByte,
    /// One comparison of heap values in a sort.
    SortComparison,
    /// One allocation from the worker's allocator.
    Allocation,
    /// One byte requested from the worker's allocator.
    AllocatedByte,
    /// One limb multiplication in a three-argument `pow()`, which also reduces by the modulus.
    ModPowProduct,
    /// One `print()` call.
    Print,
    /// One byte of an object `print()` writes.
    PrintByte,
    /// One byte hashed by `hashlib`.
    HashByte,
}

impl WorkEvent {
    /// Thousandths of a unit one occurrence is charged.
    #[inline]
    #[must_use]
    pub const fn milli_units(self) -> u64 {
        EVENT_MILLI_UNITS[self as usize] as u64
    }
}
