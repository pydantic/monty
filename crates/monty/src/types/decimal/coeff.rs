//! [`Coeff`], the coefficient of a `Decimal`: a non-negative integer stored as
//! libmpdec stores it, in little-endian words of 19 decimal digits.
//!
//! A decimal spends most of its time shifting by digits, rounding, counting digits
//! and printing. In base `10**19` each of those is a word move plus one division by
//! a power of ten below `10**19`, where a binary integer needs a full multiplication
//! or division by `10**n`. Values up to 38 digits are stored inline.
//!
//! Division by a multi-word divisor and the transcendental kernels convert to
//! `BigInt` ([`Coeff::to_bigint`], [`Coeff::from_biguint`]), which costs one short
//! multiplication or division per word.

use std::{cmp::Ordering, fmt, iter};

use num_bigint::{BigInt, BigUint, Sign};
use num_integer::Integer;
use num_traits::ToPrimitive;
use smallvec::{SmallVec, smallvec};

/// Decimal digits per word.
const WORD_DIGITS: u64 = 19;

/// The word base, `10**19`. Above `2**63`, so it is already normalized for
/// [`div_radix`]'s division by an invariant integer.
const RADIX: u64 = 10_000_000_000_000_000_000;

/// `POW10[n] == 10**n` for every digit count a word holds, `0..=19`.
const POW10: [u64; 20] = {
    let mut table = [1u64; 20];
    let mut i = 1;
    while i < table.len() {
        table[i] = table[i - 1] * 10;
        i += 1;
    }
    table
};

/// The reciprocal of [`RADIX`] for [`div_radix`]: `floor((2**128 - 1) / RADIX) - 2**64`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the reciprocal is below 2**64 by construction"
)]
const RADIX_RECIPROCAL: u64 = (u128::MAX / RADIX as u128 - (1u128 << 64)) as u64;

/// A non-negative integer in base `10**19`.
///
/// Always normalized, so the derived equality is numeric equality.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Coeff {
    /// Words below [`RADIX`], least significant first, with no zero high word; empty for zero.
    words: SmallVec<[u64; 2]>,
}

/// The digits a right shift discarded, as rounding needs them (libmpdec's rounding
/// indicator): the first discarded digit and whether any digit below it is non-zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Discarded {
    /// The most significant discarded digit.
    pub(crate) first: u8,
    /// Whether any discarded digit below `first` is non-zero.
    pub(crate) sticky: bool,
}

impl Discarded {
    /// Nothing discarded, or only zeros.
    pub(crate) const NONE: Self = Self {
        first: 0,
        sticky: false,
    };

    /// Whether every discarded digit was zero, so the shift was exact.
    pub(crate) fn is_zero(self) -> bool {
        self.first == 0 && !self.sticky
    }

    /// Whether the discarded digits are exactly one half: a `5` followed by zeros.
    pub(crate) fn is_half(self) -> bool {
        self.first == 5 && !self.sticky
    }
}

impl Coeff {
    /// Zero.
    pub(crate) fn zero() -> Self {
        Self::default()
    }

    /// One.
    pub(crate) fn one() -> Self {
        Self { words: smallvec![1] }
    }

    /// A machine integer.
    pub(crate) fn from_u64(value: u64) -> Self {
        let words = if value == 0 {
            SmallVec::new()
        } else if value < RADIX {
            smallvec![value]
        } else {
            smallvec![value % RADIX, value / RADIX]
        };
        Self { words }
    }

    /// A 128-bit integer.
    pub(crate) fn from_u128(value: u128) -> Self {
        let high = value >> 64;
        if high == 0 {
            Self::from_u64(u64::try_from(value).expect("high half is zero"))
        } else if high < u128::from(RADIX) {
            // The quotient fits a `u64`, so `div_radix` applies.
            let (quotient, low) = div_radix(value);
            let mut coeff = Self::from_u64(quotient);
            coeff.words.insert(0, low);
            coeff
        } else {
            let radix = u128::from(RADIX);
            let word = |v: u128| u64::try_from(v % radix).expect("a remainder is below RADIX");
            Self::from_words(smallvec![word(value), word(value / radix), word(value / radix / radix)])
        }
    }

    /// Normalizes words, dropping zero high words.
    fn from_words(mut words: SmallVec<[u64; 2]>) -> Self {
        while words.last() == Some(&0) {
            words.pop();
        }
        Self { words }
    }

    /// Parses ASCII decimal digits; leading zeros are allowed and an empty string is zero.
    pub(crate) fn from_digits(digits: &str) -> Self {
        debug_assert!(digits.bytes().all(|b| b.is_ascii_digit()), "not digits: {digits:?}");
        let words = digits
            .as_bytes()
            .rchunks(19)
            .map(|chunk| chunk.iter().fold(0u64, |acc, &b| acc * 10 + u64::from(b - b'0')))
            .collect();
        Self::from_words(words)
    }

    /// `10**n`.
    pub(crate) fn pow10(n: u64) -> Self {
        let zero_words = word_index(n / WORD_DIGITS);
        let mut words = SmallVec::with_capacity(zero_words + 1);
        words.resize(zero_words, 0);
        words.push(POW10[digit_index(n % WORD_DIGITS)]);
        Self { words }
    }

    /// `count` copies of the non-zero digit `digit`, such as `99...9` or `11...1`.
    pub(crate) fn repeat_digit(digit: u8, count: u64) -> Self {
        debug_assert!((1..=9).contains(&digit), "not a non-zero digit: {digit}");
        let repunit = |len: usize| (POW10[len] - 1) / 9 * u64::from(digit);
        let full = word_index(count / WORD_DIGITS);
        let partial = digit_index(count % WORD_DIGITS);
        let mut words = SmallVec::with_capacity(full + 1);
        words.resize(full, repunit(19));
        if partial > 0 {
            words.push(repunit(partial));
        }
        Self { words }
    }

    /// Whether this is zero.
    #[inline]
    pub(crate) fn is_zero(&self) -> bool {
        self.words.is_empty()
    }

    /// The number of decimal digits; zero has one, like `"0"`.
    #[inline]
    pub(crate) fn digits(&self) -> u64 {
        match self.words.last() {
            None => 1,
            Some(&top) => self.word_count() * WORD_DIGITS - WORD_DIGITS + word_digits(top),
        }
    }

    /// The number of words, as a `u64` digit-arithmetic operand.
    #[inline]
    fn word_count(&self) -> u64 {
        u64::try_from(self.words.len()).expect("word count fits in u64")
    }

    /// The least significant digit (zero for zero).
    #[inline]
    pub(crate) fn last_digit(&self) -> u8 {
        self.words.first().map_or(0, |&word| digit_of(word, 0))
    }

    /// The digit at `pos`, counting from the least significant as `0`; zero beyond the top.
    pub(crate) fn digit_at(&self, pos: u64) -> u8 {
        self.words
            .get(word_index(pos / WORD_DIGITS))
            .map_or(0, |&word| digit_of(word, pos % WORD_DIGITS))
    }

    /// The number of trailing zero digits; zero for zero.
    pub(crate) fn trailing_zeros(&self) -> u64 {
        let mut zeros = 0;
        for &word in &self.words {
            if word != 0 {
                return zeros + word_trailing_zeros(word);
            }
            zeros += WORD_DIGITS;
        }
        0
    }

    /// Whether this is a power of ten: a `1` followed only by zeros.
    pub(crate) fn is_power_of_ten(&self) -> bool {
        !self.is_zero() && self.trailing_zeros() == self.digits() - 1 && self.digit_at(self.digits() - 1) == 1
    }

    /// A hash of the value mixed into `seed`, for values no `int` or `float` equals.
    ///
    /// A multiplicative mix rather than SipHash: Monty's `int` hashes are the identity
    /// anyway, so this gives up no flooding resistance, and it is much cheaper per word.
    pub(crate) fn digest(&self, seed: u64) -> u64 {
        // The golden-ratio constant of Fibonacci hashing.
        const MULTIPLIER: u64 = 0x9E37_79B9_7F4A_7C15;
        self.words
            .iter()
            .fold(seed, |acc, &word| (acc.rotate_left(29) ^ word).wrapping_mul(MULTIPLIER))
    }

    /// The value as a `u64`, if it fits.
    pub(crate) fn to_u64(&self) -> Option<u64> {
        match *self.words.as_slice() {
            [] => Some(0),
            [word] => Some(word),
            [low, high] => high.checked_mul(RADIX)?.checked_add(low),
            _ => None,
        }
    }

    /// The value as a `u128`, if it fits.
    pub(crate) fn to_u128(&self) -> Option<u128> {
        if self.words.len() > 3 {
            return None;
        }
        self.words.iter().rev().try_fold(0u128, |acc, &word| {
            acc.checked_mul(u128::from(RADIX))?.checked_add(u128::from(word))
        })
    }

    /// `self * 10**n`. Callers check the result's size against the memory limit first.
    pub(crate) fn shl(&self, n: u64) -> Self {
        if self.is_zero() || n == 0 {
            return self.clone();
        }
        let zero_words = word_index(n / WORD_DIGITS);
        let shift = digit_index(n % WORD_DIGITS);
        let mut words = SmallVec::with_capacity(zero_words + self.words.len() + 1);
        words.resize(zero_words, 0);
        if shift == 0 {
            words.extend_from_slice(&self.words);
        } else {
            // Each word splits at `10**(19 - shift)`: its low part moves up within the
            // word, its high part carries into the next.
            let (low_div, low_mul) = (POW10[19 - shift], POW10[shift]);
            let mut carry = 0;
            for &word in &self.words {
                words.push(word % low_div * low_mul + carry);
                carry = word / low_div;
            }
            if carry != 0 {
                words.push(carry);
            }
        }
        Self { words }
    }

    /// `self // 10**n` and the digits that division discards.
    pub(crate) fn shr(&self, n: u64) -> (Self, Discarded) {
        if self.is_zero() || n == 0 {
            return (self.clone(), Discarded::NONE);
        }
        let digits = self.digits();
        let discarded = Discarded {
            first: self.digit_at(n - 1),
            sticky: self.any_nonzero_below(n - 1),
        };
        let kept = if n >= digits {
            Self::zero()
        } else {
            let dropped = word_index(n / WORD_DIGITS);
            let shift = digit_index(n % WORD_DIGITS);
            let src = &self.words[dropped..];
            if shift == 0 {
                Self::from_words(SmallVec::from_slice(src))
            } else {
                // Each word takes its own high digits and the low digits of the next.
                let (high_div, high_mul) = (POW10[shift], POW10[19 - shift]);
                let words = (0..src.len())
                    .map(|i| src[i] / high_div + src.get(i + 1).map_or(0, |next| next % high_div * high_mul))
                    .collect();
                Self::from_words(words)
            }
        };
        (kept, discarded)
    }

    /// Whether any digit below position `pos` is non-zero.
    fn any_nonzero_below(&self, pos: u64) -> bool {
        let full = word_index(pos / WORD_DIGITS).min(self.words.len());
        let partial = POW10[digit_index(pos % WORD_DIGITS)];
        self.words[..full].iter().any(|&word| word != 0)
            || self.words.get(full).is_some_and(|&word| word % partial != 0)
    }

    /// `self % 10**n`: the low `n` digits.
    pub(crate) fn low_digits(&self, n: u64) -> Self {
        let full = word_index(n / WORD_DIGITS);
        if full >= self.words.len() {
            return self.clone();
        }
        let mut words = SmallVec::from_slice(&self.words[..full]);
        words.push(self.words[full] % POW10[digit_index(n % WORD_DIGITS)]);
        Self::from_words(words)
    }

    /// `self + other`.
    pub(crate) fn add(&self, other: &Self) -> Self {
        let (long, short) = if self.words.len() >= other.words.len() {
            (self, other)
        } else {
            (other, self)
        };
        let mut words = SmallVec::with_capacity(long.words.len() + 1);
        let mut carry = false;
        for (i, &word) in long.words.iter().enumerate() {
            // Two words can sum past `u64::MAX`, so compare against `RADIX - addend`
            // instead of forming the sum; `addend <= RADIX`.
            let addend = short.words.get(i).copied().unwrap_or(0) + u64::from(carry);
            carry = word >= RADIX - addend;
            words.push(if carry { word - (RADIX - addend) } else { word + addend });
        }
        if carry {
            words.push(1);
        }
        Self { words }
    }

    /// `self - other`, for `self >= other`.
    pub(crate) fn sub(&self, other: &Self) -> Self {
        debug_assert!(self >= other, "negative difference");
        let mut words = SmallVec::with_capacity(self.words.len());
        let mut borrow = 0;
        for (i, &word) in self.words.iter().enumerate() {
            let subtrahend = other.words.get(i).copied().unwrap_or(0) + borrow;
            if word >= subtrahend {
                words.push(word - subtrahend);
                borrow = 0;
            } else {
                words.push(word + (RADIX - subtrahend));
                borrow = 1;
            }
        }
        Self::from_words(words)
    }

    /// Adds one in place.
    pub(crate) fn increment(&mut self) {
        for word in &mut self.words {
            if *word == RADIX - 1 {
                *word = 0;
            } else {
                *word += 1;
                return;
            }
        }
        self.words.push(1);
    }

    /// `self * other`: schoolbook in base `10**19`, Karatsuba once both operands are
    /// long, as num-bigint and libmpdec switch, so huge precisions stay subquadratic.
    pub(crate) fn mul(&self, other: &Self) -> Self {
        if self.is_zero() || other.is_zero() {
            return Self::zero();
        }
        if let ([a], [b]) = (self.words.as_slice(), other.words.as_slice()) {
            let (high, low) = div_radix(u128::from(*a) * u128::from(*b));
            return Self::from_words(smallvec![low, high]);
        }
        let (long, short) = if self.words.len() >= other.words.len() {
            (self, other)
        } else {
            (other, self)
        };
        if short.words.len() < KARATSUBA_WORDS {
            schoolbook(&long.words, &short.words)
        } else if long.words.len() >= 2 * short.words.len() {
            // Unbalanced: multiply `short` by each `short`-sized chunk of `long`.
            let chunk = short.words.len();
            long.words
                .chunks(chunk)
                .enumerate()
                .fold(Self::zero(), |acc, (i, words)| {
                    let part = Self::from_words(SmallVec::from_slice(words)).mul(short);
                    acc.add(&part.shift_words(i * chunk))
                })
        } else {
            // `(a1*B + a0)(b1*B + b0)` from three half-size products.
            let half = long.words.len() / 2;
            let (a0, a1) = long.split_words(half);
            let (b0, b1) = short.split_words(half);
            let low = a0.mul(&b0);
            let high = a1.mul(&b1);
            let middle = a0.add(&a1).mul(&b0.add(&b1)).sub(&low).sub(&high);
            high.shift_words(2 * half).add(&middle.shift_words(half)).add(&low)
        }
    }

    /// `(self % B**n, self // B**n)` for the word base `B`.
    fn split_words(&self, n: usize) -> (Self, Self) {
        let split = n.min(self.words.len());
        (
            Self::from_words(SmallVec::from_slice(&self.words[..split])),
            Self::from_words(SmallVec::from_slice(&self.words[split..])),
        )
    }

    /// `self * B**n` for the word base `B`.
    fn shift_words(&self, n: usize) -> Self {
        if self.is_zero() {
            return Self::zero();
        }
        let mut words = SmallVec::with_capacity(n + self.words.len());
        words.resize(n, 0);
        words.extend_from_slice(&self.words);
        Self { words }
    }

    /// `(self // other, self % other)` for a non-zero `other`.
    pub(crate) fn div_rem(&self, other: &Self) -> (Self, Self) {
        debug_assert!(!other.is_zero(), "division by zero");
        if self < other {
            return (Self::zero(), self.clone());
        }
        if let (Some(a), Some(b)) = (self.to_u128(), other.to_u128()) {
            return (Self::from_u128(a / b), Self::from_u128(a % b));
        }
        if let [divisor] = *other.words.as_slice() {
            // Short division, one word at a time from the top.
            let divisor = u128::from(divisor);
            let mut rem = 0u128;
            let mut words: SmallVec<[u64; 2]> = smallvec![0; self.words.len()];
            for (i, &word) in self.words.iter().enumerate().rev() {
                let cur = rem * u128::from(RADIX) + u128::from(word);
                words[i] = u64::try_from(cur / divisor).expect("a quotient word is below RADIX");
                rem = cur % divisor;
            }
            let rem = u64::try_from(rem).expect("the remainder is below the divisor");
            return (Self::from_words(words), Self::from_u64(rem));
        }
        let (q, r) = self.to_biguint().div_rem(&other.to_biguint());
        (Self::from_biguint(&q), Self::from_biguint(&r))
    }

    /// The value as a binary integer: Horner's rule, one short multiplication per word.
    pub(crate) fn to_biguint(&self) -> BigUint {
        let mut limbs: Vec<u64> = Vec::with_capacity(self.words.len());
        for &word in self.words.iter().rev() {
            let mut carry = word;
            for limb in &mut limbs {
                let t = u128::from(*limb) * u128::from(RADIX) + u128::from(carry);
                *limb = low_half(t);
                carry = high_half(t);
            }
            if carry != 0 {
                limbs.push(carry);
            }
        }
        let half = |bits: u64| u32::try_from(bits & u64::from(u32::MAX)).expect("masked below 2**32");
        BigUint::new(limbs.iter().flat_map(|&limb| [half(limb), half(limb >> 32)]).collect())
    }

    /// The value as a signed binary integer.
    pub(crate) fn to_bigint(&self) -> BigInt {
        BigInt::from_biguint(Sign::Plus, self.to_biguint())
    }

    /// A binary integer, by repeated short division by `10**19`.
    pub(crate) fn from_biguint(value: &BigUint) -> Self {
        if let Some(small) = value.to_u128() {
            return Self::from_u128(small);
        }
        let mut limbs = value.to_u64_digits();
        let mut words = SmallVec::with_capacity(limbs.len() * 20 / 19 + 1);
        while !limbs.is_empty() {
            let mut rem = 0;
            for limb in limbs.iter_mut().rev() {
                let (quotient, r) = div_radix((u128::from(rem) << 64) | u128::from(*limb));
                *limb = quotient;
                rem = r;
            }
            words.push(rem);
            while limbs.last() == Some(&0) {
                limbs.pop();
            }
        }
        Self::from_words(words)
    }

    /// The magnitude of a binary integer.
    pub(crate) fn from_bigint(value: &BigInt) -> Self {
        Self::from_biguint(value.magnitude())
    }
}

impl Ord for Coeff {
    fn cmp(&self, other: &Self) -> Ordering {
        self.words
            .len()
            .cmp(&other.words.len())
            .then_with(|| self.words.iter().rev().cmp(other.words.iter().rev()))
    }
}

impl PartialOrd for Coeff {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Coeff {
    /// Appends the decimal digits to `out`, without the formatting machinery `Display`
    /// goes through: `str()` of a `Decimal` is hot.
    pub(crate) fn write_digits(&self, out: &mut String) {
        let mut words = self.words.iter().rev();
        let Some(&top) = words.next() else {
            out.push('0');
            return;
        };
        out.reserve(digit_index(word_digits(top)) + self.words.len() * 19 - 19);
        let mut buf = itoa::Buffer::new();
        out.push_str(buf.format(top));
        for &word in words {
            let digits = buf.format(word);
            // Every word below the top is zero-padded to its 19 digits.
            out.extend(iter::repeat_n('0', 19 - digits.len()));
            out.push_str(digits);
        }
    }
}

impl fmt::Display for Coeff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut digits = String::new();
        self.write_digits(&mut digits);
        f.write_str(&digits)
    }
}

/// The shorter operand's word count from which [`Coeff::mul`] uses Karatsuba.
const KARATSUBA_WORDS: usize = 48;

/// Schoolbook multiplication of two non-empty word slices.
fn schoolbook(a: &[u64], b: &[u64]) -> Coeff {
    let mut words: SmallVec<[u64; 2]> = smallvec![0; a.len() + b.len()];
    for (i, &x) in a.iter().enumerate() {
        let mut carry = 0;
        for (j, &y) in b.iter().enumerate() {
            // At most `(R-1)**2 + 2(R-1) < R**2`, so the quotient fits a word.
            let t = u128::from(x) * u128::from(y) + u128::from(words[i + j]) + u128::from(carry);
            let (high, low) = div_radix(t);
            words[i + j] = low;
            carry = high;
        }
        words[i + b.len()] = carry;
    }
    Coeff::from_words(words)
}

/// Serialized as its words; a dump is trusted, but zero high words are still dropped.
impl serde::Serialize for Coeff {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(&self.words)
    }
}

impl<'de> serde::Deserialize<'de> for Coeff {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_words(SmallVec::deserialize(deserializer)?))
    }
}

/// Divides `t` by [`RADIX`] for `t < RADIX * 2**64`, returning `(quotient, remainder)`.
///
/// Möller and Granlund's division by an invariant integer: two multiplications and
/// two corrections, avoiding the software 128-bit division a plain `/` compiles to.
fn div_radix(t: u128) -> (u64, u64) {
    let (u1, u0) = (high_half(t), low_half(t));
    debug_assert!(u1 < RADIX, "quotient does not fit a word");
    let product = u128::from(RADIX_RECIPROCAL) * u128::from(u1) + t;
    let mut q = high_half(product).wrapping_add(1);
    let mut r = u0.wrapping_sub(q.wrapping_mul(RADIX));
    if r > low_half(product) {
        q = q.wrapping_sub(1);
        r = r.wrapping_add(RADIX);
    }
    if r >= RADIX {
        q += 1;
        r -= RADIX;
    }
    debug_assert_eq!(
        (q, r),
        (low_half(t / u128::from(RADIX)), low_half(t % u128::from(RADIX)))
    );
    (q, r)
}

/// The high 64 bits.
fn high_half(t: u128) -> u64 {
    u64::try_from(t >> 64).expect("shifted below 2**64")
}

/// The low 64 bits.
fn low_half(t: u128) -> u64 {
    u64::try_from(t & u128::from(u64::MAX)).expect("masked below 2**64")
}

/// The number of decimal digits of a non-zero word.
#[inline]
fn word_digits(word: u64) -> u64 {
    u64::from(word.ilog10()) + 1
}

/// The number of trailing zero digits of a non-zero word.
fn word_trailing_zeros(mut word: u64) -> u64 {
    let mut zeros = 0;
    while word.is_multiple_of(10) {
        word /= 10;
        zeros += 1;
    }
    zeros
}

/// The digit of `word` at `pos` (`0..19`), counting from the least significant.
fn digit_of(word: u64, pos: u64) -> u8 {
    u8::try_from(word / POW10[digit_index(pos)] % 10).expect("a digit is below ten")
}

/// A word index from a digit-derived count; one beyond memory saturates, so it
/// compares past the end of any coefficient.
fn word_index(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// An index into [`POW10`] for a digit position within a word.
fn digit_index(n: u64) -> usize {
    usize::try_from(n).expect("a digit position within a word is below 20")
}
