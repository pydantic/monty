//! Arithmetic contexts, rounding modes and the signals an operation can raise.
//!
//! [`DecContext`] is the plain data behind a Python `decimal.Context`. An operation
//! runs against a [`Ctx`]: a copy of those settings plus the conditions raised so far.
//! Conditions are only *recorded* while the operation runs; the caller applies them
//! to the context object afterwards ([`Conditions::trapped`]), as C `_decimal` does,
//! so every flag one operation raises is set before a trapped one is reported.

use std::fmt::{self, Write};

use monty_types::{ExcType, ResourceTracker};

use crate::exception_private::RunResult;

/// C `_decimal`'s `MAX_PREC` (64-bit builds), the largest settable `prec`.
pub(crate) const MAX_PREC: i64 = 999_999_999_999_999_999;
/// C `_decimal`'s `MAX_EMAX`, the largest settable `Emax`.
pub(crate) const MAX_EMAX: i64 = 999_999_999_999_999_999;
/// C `_decimal`'s `MIN_EMIN`, the smallest settable `Emin`.
pub(crate) const MIN_EMIN: i64 = -999_999_999_999_999_999;
/// C `_decimal`'s `MIN_ETINY`: `MIN_EMIN - (MAX_PREC - 1)`.
pub(crate) const MIN_ETINY: i64 = MIN_EMIN - (MAX_PREC - 1);

/// The eight rounding modes, in the order C `_decimal` lists them in its error message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Rounding {
    Ceiling,
    Floor,
    Up,
    Down,
    HalfUp,
    HalfDown,
    HalfEven,
    Up05,
}

impl Rounding {
    /// Every mode, for the module constants.
    pub(crate) const ALL: [Self; 8] = [
        Self::Ceiling,
        Self::Floor,
        Self::Up,
        Self::Down,
        Self::HalfUp,
        Self::HalfDown,
        Self::HalfEven,
        Self::Up05,
    ];

    /// The `ROUND_*` string Python code uses for this mode.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Ceiling => "ROUND_CEILING",
            Self::Floor => "ROUND_FLOOR",
            Self::Up => "ROUND_UP",
            Self::Down => "ROUND_DOWN",
            Self::HalfUp => "ROUND_HALF_UP",
            Self::HalfDown => "ROUND_HALF_DOWN",
            Self::HalfEven => "ROUND_HALF_EVEN",
            Self::Up05 => "ROUND_05UP",
        }
    }

    /// Parses a `ROUND_*` string; `None` for anything else.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.name() == name)
    }
}

/// The nine signals, in C `_decimal`'s `signal_map` order.
///
/// That order decides which exception class a trapped operation raises (the first
/// trapped signal) and how a `SignalDict` iterates and prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Signal {
    InvalidOperation,
    FloatOperation,
    DivisionByZero,
    Overflow,
    Underflow,
    Subnormal,
    Inexact,
    Rounded,
    Clamped,
}

impl Signal {
    /// Every signal in `signal_map` order.
    pub(crate) const ALL: [Self; 9] = [
        Self::InvalidOperation,
        Self::FloatOperation,
        Self::DivisionByZero,
        Self::Overflow,
        Self::Underflow,
        Self::Subnormal,
        Self::Inexact,
        Self::Rounded,
        Self::Clamped,
    ];

    /// The exception type standing for this signal.
    pub(crate) fn exc_type(self) -> ExcType {
        match self {
            Self::InvalidOperation => ExcType::DecimalInvalidOperation,
            Self::FloatOperation => ExcType::DecimalFloatOperation,
            Self::DivisionByZero => ExcType::DecimalDivisionByZero,
            Self::Overflow => ExcType::DecimalOverflow,
            Self::Underflow => ExcType::DecimalUnderflow,
            Self::Subnormal => ExcType::DecimalSubnormal,
            Self::Inexact => ExcType::DecimalInexact,
            Self::Rounded => ExcType::DecimalRounded,
            Self::Clamped => ExcType::DecimalClamped,
        }
    }

    /// The signal an exception type stands for, if it is one of the nine.
    ///
    /// The `InvalidOperation` subclasses are conditions, not signals, so they
    /// return `None`: `ctx.traps[ConversionSyntax]` is a `KeyError` in CPython.
    pub(crate) fn from_exc_type(exc_type: ExcType) -> Option<Self> {
        Self::ALL.into_iter().find(|signal| signal.exc_type() == exc_type)
    }

    /// The conditions this signal stands for in flags and traps: `InvalidOperation`
    /// covers its four sub-conditions, every other signal only itself.
    pub(crate) fn conditions(self) -> Conditions {
        match self {
            Self::InvalidOperation => Conditions::of(&[
                Condition::InvalidOperation,
                Condition::ConversionSyntax,
                Condition::DivisionImpossible,
                Condition::DivisionUndefined,
                Condition::InvalidContext,
            ]),
            Self::FloatOperation => Conditions::of(&[Condition::FloatOperation]),
            Self::DivisionByZero => Conditions::of(&[Condition::DivisionByZero]),
            Self::Overflow => Conditions::of(&[Condition::Overflow]),
            Self::Underflow => Conditions::of(&[Condition::Underflow]),
            Self::Subnormal => Conditions::of(&[Condition::Subnormal]),
            Self::Inexact => Conditions::of(&[Condition::Inexact]),
            Self::Rounded => Conditions::of(&[Condition::Rounded]),
            Self::Clamped => Conditions::of(&[Condition::Clamped]),
        }
    }
}

/// What an operation can raise: the nine signals plus the four `InvalidOperation`
/// sub-conditions, declared in libmpdec's status-bit order.
///
/// Flags and traps are stored per condition, as libmpdec stores them: setting a
/// signal sets every condition it covers, and `Context.__repr__` prints the
/// conditions in this order, so which sub-condition raised `InvalidOperation`
/// decides where it is listed. Signal-level reads test any covered condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Condition {
    Clamped,
    ConversionSyntax,
    DivisionByZero,
    DivisionImpossible,
    DivisionUndefined,
    Inexact,
    InvalidContext,
    InvalidOperation,
    FloatOperation,
    Overflow,
    Rounded,
    Subnormal,
    Underflow,
}

impl Condition {
    /// Every condition in libmpdec bit order, which `Context.__repr__` follows.
    const ALL: [Self; 13] = [
        Self::Clamped,
        Self::ConversionSyntax,
        Self::DivisionByZero,
        Self::DivisionImpossible,
        Self::DivisionUndefined,
        Self::Inexact,
        Self::InvalidContext,
        Self::InvalidOperation,
        Self::FloatOperation,
        Self::Overflow,
        Self::Rounded,
        Self::Subnormal,
        Self::Underflow,
    ];

    /// The order a trapped operation's message lists conditions in: C `_decimal`'s
    /// `cond_map`, then the remaining signals in `signal_map` order.
    const MESSAGE_ORDER: [Self; 13] = [
        Self::InvalidOperation,
        Self::ConversionSyntax,
        Self::DivisionImpossible,
        Self::DivisionUndefined,
        Self::InvalidContext,
        Self::FloatOperation,
        Self::DivisionByZero,
        Self::Overflow,
        Self::Underflow,
        Self::Subnormal,
        Self::Inexact,
        Self::Rounded,
        Self::Clamped,
    ];

    /// The signal this condition sets and is trapped by.
    pub(crate) fn signal(self) -> Signal {
        match self {
            Self::InvalidOperation
            | Self::ConversionSyntax
            | Self::DivisionImpossible
            | Self::DivisionUndefined
            | Self::InvalidContext => Signal::InvalidOperation,
            Self::FloatOperation => Signal::FloatOperation,
            Self::DivisionByZero => Signal::DivisionByZero,
            Self::Overflow => Signal::Overflow,
            Self::Underflow => Signal::Underflow,
            Self::Subnormal => Signal::Subnormal,
            Self::Inexact => Signal::Inexact,
            Self::Rounded => Signal::Rounded,
            Self::Clamped => Signal::Clamped,
        }
    }

    /// The exception class naming this condition in a trap message.
    fn exc_type(self) -> ExcType {
        match self {
            Self::ConversionSyntax => ExcType::DecimalConversionSyntax,
            Self::DivisionImpossible => ExcType::DecimalDivisionImpossible,
            Self::DivisionUndefined => ExcType::DecimalDivisionUndefined,
            Self::InvalidContext => ExcType::DecimalInvalidContext,
            other => other.signal().exc_type(),
        }
    }

    /// The bit this condition occupies in a [`Conditions`] set.
    const fn bit(self) -> u16 {
        1 << self as u8
    }
}

/// A set of [`Condition`]s: an operation's status, or a context's flags or traps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub(crate) struct Conditions(u16);

impl Conditions {
    /// No conditions.
    pub(crate) const EMPTY: Self = Self(0);

    /// Builds a set from a list of conditions.
    pub(crate) const fn of(conditions: &[Condition]) -> Self {
        let mut bits = 0;
        let mut i = 0;
        while i < conditions.len() {
            bits |= conditions[i].bit();
            i += 1;
        }
        Self(bits)
    }

    /// Records `condition`.
    pub(crate) fn insert(&mut self, condition: Condition) {
        self.0 |= condition.bit();
    }

    /// Whether `condition` was recorded.
    pub(crate) fn contains(self, condition: Condition) -> bool {
        self.0 & condition.bit() != 0
    }

    /// Adds every condition of `other`.
    pub(crate) fn merge(&mut self, other: Self) {
        self.0 |= other.0;
    }

    /// Whether the set is empty.
    pub(crate) fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether any condition `signal` covers is in the set (`flags[signal]`).
    pub(crate) fn has_signal(self, signal: Signal) -> bool {
        self.0 & signal.conditions().0 != 0
    }

    /// Sets or clears every condition `signal` covers (`flags[signal] = on`).
    pub(crate) fn set_signal(&mut self, signal: Signal, on: bool) {
        if on {
            self.0 |= signal.conditions().0;
        } else {
            self.0 &= !signal.conditions().0;
        }
    }

    /// Writes the signals as `Context.__repr__` does, in libmpdec bit order with
    /// `InvalidOperation` listed once: `[Inexact, InvalidOperation, Rounded]`.
    pub(crate) fn write_names(self, f: &mut impl Write) -> fmt::Result {
        f.write_char('[')?;
        let mut first = true;
        let mut invalid_written = false;
        for condition in Condition::ALL.into_iter().filter(|c| self.contains(*c)) {
            let signal = condition.signal();
            if signal == Signal::InvalidOperation {
                if invalid_written {
                    continue;
                }
                invalid_written = true;
            }
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            let name: &'static str = signal.exc_type().into();
            f.write_str(name.strip_prefix("decimal.").unwrap_or(name))?;
        }
        f.write_char(']')
    }

    /// The exception a trapped operation raises, or `None` if no recorded
    /// condition is trapped by `traps`.
    ///
    /// The class is the first trapped signal in `signal_map` order; the message lists
    /// every trapped condition, as C `_decimal` does: `[<class 'decimal.DivisionByZero'>]`.
    pub(crate) fn trapped(self, traps: Self) -> Option<(ExcType, String)> {
        let trapped = Self(self.0 & traps.0);
        let first = Signal::ALL.into_iter().find(|signal| trapped.has_signal(*signal))?;
        let mut message = String::from("[");
        let listed = Condition::MESSAGE_ORDER.into_iter().filter(|c| trapped.contains(*c));
        for (i, condition) in listed.enumerate() {
            if i > 0 {
                message.push_str(", ");
            }
            let name: &'static str = condition.exc_type().into();
            write!(message, "<class '{name}'>").expect("writing to a String cannot fail");
        }
        message.push(']');
        Some((first.exc_type(), message))
    }
}

/// The settings and flags of a Python `decimal.Context`.
///
/// Stored on the heap as `HeapData::DecimalContext`; operations copy it into a [`Ctx`].
/// Field ranges are validated by the Python-facing setters, so the arithmetic may rely
/// on `1 <= prec <= MAX_PREC`, `MIN_EMIN <= emin <= 0 <= emax <= MAX_EMAX`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct DecContext {
    pub(crate) prec: i64,
    pub(crate) rounding: Rounding,
    pub(crate) emin: i64,
    pub(crate) emax: i64,
    /// `True` prints exponents with `E`, `False` with `e`.
    pub(crate) capitals: bool,
    /// IEEE clamping: exponents above `Etop` fold into the coefficient.
    pub(crate) clamp: bool,
    pub(crate) flags: Conditions,
    pub(crate) traps: Conditions,
}

impl DecContext {
    /// `decimal.DefaultContext`, which a new thread's (here: session's) context copies.
    pub(crate) const DEFAULT: Self = Self {
        prec: 28,
        rounding: Rounding::HalfEven,
        emin: -999_999,
        emax: 999_999,
        capitals: true,
        clamp: false,
        flags: Conditions::EMPTY,
        traps: Conditions::of(&[
            // `InvalidOperation` and its sub-conditions.
            Condition::InvalidOperation,
            Condition::ConversionSyntax,
            Condition::DivisionImpossible,
            Condition::DivisionUndefined,
            Condition::InvalidContext,
            Condition::DivisionByZero,
            Condition::Overflow,
        ]),
    };

    /// `decimal.BasicContext`.
    pub(crate) const BASIC: Self = Self {
        prec: 9,
        rounding: Rounding::HalfUp,
        traps: Conditions::of(&[
            Condition::InvalidOperation,
            Condition::ConversionSyntax,
            Condition::DivisionImpossible,
            Condition::DivisionUndefined,
            Condition::InvalidContext,
            Condition::DivisionByZero,
            Condition::Overflow,
            Condition::Underflow,
            Condition::Clamped,
        ]),
        ..Self::DEFAULT
    };

    /// `decimal.ExtendedContext`.
    pub(crate) const EXTENDED: Self = Self {
        prec: 9,
        traps: Conditions::EMPTY,
        ..Self::DEFAULT
    };

    /// `Etiny = Emin - prec + 1`, the smallest exponent of a subnormal result.
    pub(crate) fn etiny(&self) -> i64 {
        self.emin - self.prec + 1
    }

    /// `Etop = Emax - prec + 1`, the largest exponent of a full-precision result.
    pub(crate) fn etop(&self) -> i64 {
        self.emax - self.prec + 1
    }

    /// Writes `Context(prec=28, rounding=ROUND_HALF_EVEN, ...)`.
    pub(crate) fn write_repr(&self, f: &mut impl Write) -> fmt::Result {
        write!(
            f,
            "Context(prec={}, rounding={}, Emin={}, Emax={}, capitals={}, clamp={}, flags=",
            self.prec,
            self.rounding.name(),
            self.emin,
            self.emax,
            u8::from(self.capitals),
            u8::from(self.clamp)
        )?;
        self.flags.write_names(f)?;
        f.write_str(", traps=")?;
        self.traps.write_names(f)?;
        f.write_char(')')
    }
}

/// The working state of one operation: the context settings it runs under, the
/// conditions raised so far and the tracker that bounds its work.
///
/// Ported code reads settings through the field (`ctx.settings.prec`) and records
/// signals with [`Ctx::raise`] or one of the helpers returning the signal's default
/// result. Nothing here raises: see the module docs.
pub(crate) struct Ctx<'a> {
    /// The context settings; ported code may change `rounding` or `prec` locally,
    /// as `_pydecimal` does on a shallow copy.
    pub(crate) settings: DecContext,
    /// Conditions raised so far.
    pub(crate) status: Conditions,
    /// Bounds the size of intermediate results and the time spent in long loops.
    pub(crate) tracker: &'a ResourceTracker,
}

impl<'a> Ctx<'a> {
    /// A fresh operation under `settings`.
    pub(crate) fn new(settings: DecContext, tracker: &'a ResourceTracker) -> Self {
        Self {
            settings,
            status: Conditions::default(),
            tracker,
        }
    }

    /// The same settings with a different rounding mode and an empty status, for
    /// sub-computations whose conditions the caller decides whether to keep.
    pub(crate) fn with_rounding(&self, rounding: Rounding) -> Self {
        let mut settings = self.settings;
        settings.rounding = rounding;
        Ctx::new(settings, self.tracker)
    }

    /// Records `condition` (`context._raise_error` for signals whose default
    /// handling leaves the result unchanged).
    pub(crate) fn raise(&mut self, condition: Condition) {
        self.status.insert(condition);
    }

    /// Fails with `MemoryError` if materializing a coefficient of `digits` decimal
    /// digits would exceed the memory limit. Cheap below a few thousand digits.
    pub(crate) fn check_digits(&self, digits: u64) -> RunResult<()> {
        // ~0.42 bytes per digit in a binary integer, doubled for its decimal string.
        const BIG_DIGITS: u64 = 1 << 16;
        if digits > BIG_DIGITS {
            let bytes = usize::try_from(digits).unwrap_or(usize::MAX);
            self.tracker.check_allocation(bytes)?;
        }
        Ok(())
    }

    /// Polls the time and memory limits from a loop whose trip count grows with the
    /// precision (`_pydecimal`'s refinement and Newton loops).
    pub(crate) fn check_time(&self) -> RunResult<()> {
        Ok(self.tracker.check_memory_time()?)
    }
}
