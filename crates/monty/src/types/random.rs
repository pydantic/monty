//! The `random.Random` generator: CPython's MT19937 core plus the seeding and
//! state that `random.py` adds to it.
//!
//! [`Mt19937`] reproduces `Modules/_randommodule.c` bit for bit (`init_by_array`
//! seeding, `random()`'s 53-bit float, `getrandbits()`'s word packing), so a
//! seeded Monty generator yields exactly CPython's sequence. [`Random`] adds
//! `gauss()`'s cached second value and an *unseeded* state: an unseeded
//! generator takes its first state from the session's [`RandomStart`] on its
//! first draw (see [`SessionRandom::first_state`]). The module-level functions
//! use the generator stored on the VM ([`RandomTarget::Global`]);
//! `random.Random(...)` instances are stored on the heap.

use std::{fmt::Write, mem};

use monty_types::{RandomSeed, RandomStart, ResourceTracker};
use num_bigint::{BigInt, BigUint};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, hash_python_bytes, hash_python_str, identity_hash},
    heap::{DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead, HeapRead, HeapReadOutput},
    intern::StaticStrings,
    modules::{
        copy::{Memo, PyDeepCopy},
        random::{RandomFunctions, random_dispatch},
    },
    types::{LazyHeapSet, PyTrait, Type, py_trait::PyObjectIdentity},
    value::{EitherStr, Value},
};

/// Words in the MT19937 state vector.
const N: usize = 624;
/// The twist offset.
const M: usize = 397;
const MATRIX_A: u32 = 0x9908_b0df;
const UPPER_MASK: u32 = 0x8000_0000;
const LOWER_MASK: u32 = 0x7fff_ffff;

/// Bytes of entropy an unseeded generator reads (or asks the host for under
/// `CallHost`): one full state vector, exactly what CPython's
/// `random_seed_urandom` reads.
pub(crate) const SEED_BYTES: usize = N * 4;

/// The session's `random` state: the module-level generator, and where
/// unseeded generators get their first state from. Carried between REPL
/// snippets like the globals and included in a dump.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct SessionRandom {
    /// The generator behind `random.random()` and friends.
    pub(crate) module: Random,
    /// Under [`RandomStart::Seed`], the stream unseeded instances and `seed()`
    /// take their states from, derived from the session seed on first use so
    /// each such state is deterministic yet distinct from the module's.
    derived: Option<Mt19937>,
}

impl SessionRandom {
    /// The state an unseeded `target` starts with: exactly `random.seed(s)` for
    /// the module generator under `Seed(s)`, otherwise a fresh state. `None`
    /// is `CallHost`: the state is the host's to supply.
    pub(crate) fn first_state(&mut self, target: RandomTarget, start: &RandomStart) -> Option<Mt19937> {
        match (start, target) {
            (RandomStart::Seed(seed), RandomTarget::Global) => Some(Mt19937::from_key(&seed_key_from_seed(seed))),
            _ => self.fresh_state(start),
        }
    }

    /// A state for `seed()` / `seed(None)` or an unseeded instance: OS entropy,
    /// or under `Seed(s)` the next state of the stream derived from `s`.
    /// `None` is `CallHost`: the state is the host's to supply.
    pub(crate) fn fresh_state(&mut self, start: &RandomStart) -> Option<Mt19937> {
        match start {
            RandomStart::CallHost => None,
            RandomStart::Random => Some(Mt19937::from_os_entropy()),
            RandomStart::Seed(seed) => {
                let stream = self.derived.get_or_insert_with(|| {
                    // One extra word keeps the stream distinct from `seed(s)`'s own state.
                    let mut key = seed_key_from_seed(seed);
                    key.push(DERIVED_STREAM_TAG);
                    Mt19937::from_key(&key)
                });
                let words: Vec<u32> = (0..N).map(|_| stream.next_u32()).collect();
                Some(Mt19937::from_key(&words))
            }
        }
    }
}

/// Appended to a session seed's key for the derived stream (`SessionRandom::derived`).
const DERIVED_STREAM_TAG: u32 = 0x6d6f_6e74; // "mont"

/// A `random.Random` generator, seeded or not yet.
///
/// `None` in `rng` is the *unseeded* state: `random.Random()` and the
/// module-level generator start here and stay here until a seed is given or
/// the first draw takes one from the session's [`RandomStart`]. Serialized as
/// VM/heap state, so a seeded generator survives a dump, a REPL feed boundary,
/// and a mid-call suspension.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Random {
    /// `None` until explicitly seeded, restored with `setstate()`, or
    /// initialized on the first draw. Also temporarily `None` while
    /// [`RandomTarget::with_generator`] holds the state outside its owner.
    rng: Option<Mt19937>,
    /// `gauss()`'s spare deviate, cleared by every reseed as CPython does.
    gauss_next: Option<f64>,
}

impl Random {
    /// A generator already seeded with `rng`.
    pub(crate) fn seeded(rng: Mt19937) -> Self {
        Self {
            rng: Some(rng),
            gauss_next: None,
        }
    }

    /// Whether a draw can proceed without taking a first state.
    pub(crate) fn is_seeded(&self) -> bool {
        self.rng.is_some()
    }

    /// Installs a new core state, dropping the cached gaussian deviate.
    pub(crate) fn reseed(&mut self, rng: Mt19937) {
        *self = Self::seeded(rng);
    }

    /// The core generator; callers check [`Self::is_seeded`] first.
    pub(crate) fn rng(&mut self) -> &mut Mt19937 {
        self.rng.as_mut().expect("random generator drawn from while unseeded")
    }

    /// Takes `gauss()`'s spare value, if one is cached.
    pub(crate) fn take_gauss_next(&mut self) -> Option<f64> {
        self.gauss_next.take()
    }

    /// `gauss()`'s cached spare value, as `getstate()` reports it.
    pub(crate) fn gauss_next(&self) -> Option<f64> {
        self.gauss_next
    }

    /// Caches the second deviate `gauss()` produced.
    pub(crate) fn set_gauss_next(&mut self, value: Option<f64>) {
        self.gauss_next = value;
    }

    /// `random.Random(x=None)`: `x` seeds like `seed(x)`; `None` (the default)
    /// leaves the instance unseeded until its first draw.
    pub(crate) fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let RandomInitArgs { x } = RandomInitArgs::from_args(args, vm)?;
        defer_drop!(x, vm);
        let random = if matches!(x, Value::None) {
            Self::default()
        } else {
            Self::seeded(Mt19937::from_key(&seed_key_from_value(x, SEED_VERSION_DEFAULT, vm)?))
        };
        Ok(Value::Ref(vm.heap.allocate(HeapData::Random(Box::new(random)))))
    }
}

/// `Random(x=None)` — `random.Random.__init__` is a plain Python `def`.
#[derive(FromArgs)]
#[from_args(name = "Random", style = def)]
struct RandomInitArgs {
    #[from_args(default = Value::None)]
    x: Value,
}

/// The `version` `seed()` uses when none is given.
pub(crate) const SEED_VERSION_DEFAULT: i64 = 2;

/// Which generator a `random` operation acts on: the module-level one on the
/// VM, or a `random.Random` instance on the heap.
///
/// Carried by the entropy suspension (`PostConversionEffect::SeedRandom`) so
/// the resume can seed the right generator; the instance form holds no
/// reference of its own — the effect pins the object across the yield.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum RandomTarget {
    /// The module-level generator behind `random.random()` and friends.
    Global,
    /// A `random.Random` instance.
    Instance(HeapId),
}

impl RandomTarget {
    /// Whether the target already has a state to draw from.
    pub(crate) fn is_seeded(self, vm: &VM<'_>) -> bool {
        match self {
            Self::Global => vm.random.module.is_seeded(),
            Self::Instance(id) => match vm.heap.get(id) {
                HeapData::Random(random) => random.is_seeded(),
                _ => unreachable!("RandomTarget::Instance points at a non-Random heap entry"),
            },
        }
    }

    /// Runs `f` with the target's generator lifted out of the VM (or heap) and
    /// put back afterwards, so the closure can use `vm` freely alongside it.
    ///
    /// A nested call reaching the *same* generator inside `f` sees it as
    /// unseeded, so draw first and only then run anything that may re-enter
    /// Python (`__getitem__`, comparisons).
    pub(crate) fn with_generator<'h, T>(self, vm: &mut VM<'h>, f: impl FnOnce(&mut Random, &mut VM<'h>) -> T) -> T {
        let mut random = self.take(vm);
        let result = f(&mut random, vm);
        self.restore(vm, random);
        result
    }

    /// Installs `rng` as the target's core state, as `seed()` does.
    pub(crate) fn reseed(self, vm: &mut VM<'_>, rng: Mt19937) {
        self.with_generator(vm, |random, _| random.reseed(rng));
    }

    /// Replaces the target's generator with `random`.
    fn restore(self, vm: &mut VM<'_>, random: Random) {
        match self {
            Self::Global => vm.random.module = random,
            Self::Instance(id) => match vm.heap.read(id) {
                HeapReadOutput::Random(mut handle) => *handle.get_mut(vm.heap) = random,
                _ => unreachable!("RandomTarget::Instance points at a non-Random heap entry"),
            },
        }
    }

    /// Lifts the generator out, leaving an unseeded placeholder behind.
    fn take(self, vm: &mut VM<'_>) -> Random {
        match self {
            Self::Global => mem::take(&mut vm.random.module),
            Self::Instance(id) => match vm.heap.read(id) {
                HeapReadOutput::Random(mut handle) => mem::take(handle.get_mut(vm.heap)),
                _ => unreachable!("RandomTarget::Instance points at a non-Random heap entry"),
            },
        }
    }
}

// ============================================================================
// Seeding
// ============================================================================

/// The `init_by_array` key CPython's `random_seed` derives from a non-`None`
/// seed, with `random.py`'s `str`/`bytes` preprocessing for `version` applied
/// first.
///
/// Ints use their absolute value; floats CPython's `hash()`; `str`/`bytes` are
/// SHA-512 extended (version 2) or folded by the version 1 loop. Any other type
/// is rejected with `random.py`'s message.
pub(crate) fn seed_key_from_value(value: &Value, version: i64, vm: &VM<'_>) -> RunResult<Vec<u32>> {
    let key = match value {
        Value::Bool(b) => vec![u32::from(*b)],
        Value::Int(i) => key_from_u64(i.unsigned_abs()),
        Value::Float(f) => key_from_u64(cpython_float_hash(*f)),
        Value::InternString(id) => key_from_text(&SeedText::Str(vm.interns.get_str(*id)), version),
        Value::InternBytes(id) => key_from_text(&SeedText::Bytes(vm.interns.get_bytes(*id)), version),
        Value::InternLongInt(id) => key_from_bigint(vm.interns.get_long_int(*id)),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::LongInt(li) => key_from_bigint(li.inner()),
            HeapData::Str(s) => key_from_text(&SeedText::Str(s.as_str()), version),
            HeapData::Bytes(b) => key_from_text(&SeedText::Bytes(b.as_slice()), version),
            _ => return Err(ExcType::random_seed_type()),
        },
        _ => return Err(ExcType::random_seed_type()),
    };
    Ok(key)
}

/// The key a host-chosen [`RandomSeed`] reduces to: what `random.seed(seed)`
/// would build, with the default `version` of 2.
fn seed_key_from_seed(seed: &RandomSeed) -> Vec<u32> {
    match seed {
        RandomSeed::Int(n) => key_from_bigint(n),
        RandomSeed::Float(f) => key_from_u64(cpython_float_hash(*f)),
        RandomSeed::Str(s) => key_from_text(&SeedText::Str(s), SEED_VERSION_DEFAULT),
        RandomSeed::Bytes(b) => key_from_text(&SeedText::Bytes(b), SEED_VERSION_DEFAULT),
    }
}

/// A `str` or `bytes` seed, which `random.py` treats alike apart from encoding.
enum SeedText<'a> {
    Str(&'a str),
    Bytes(&'a [u8]),
}

impl SeedText<'_> {
    /// The code units the version 1 loop folds: code points for `str`, and
    /// the bytes themselves for `bytes` (its `latin-1` decode).
    fn units(&self) -> Vec<u64> {
        match self {
            Self::Str(s) => s.chars().map(u64::from).collect(),
            Self::Bytes(b) => b.iter().map(|b| u64::from(*b)).collect(),
        }
    }

    /// The bytes version 2 hashes: a `str` is UTF-8 encoded first.
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Str(s) => s.as_bytes(),
            Self::Bytes(b) => b,
        }
    }

    /// Monty's own `hash()` of the seed, for a `version` CPython would hash
    /// (randomized per process there, deterministic here).
    fn hash(&self) -> u64 {
        match self {
            Self::Str(s) => hash_python_str(s),
            Self::Bytes(b) => hash_python_bytes(b),
        }
        .raw()
    }
}

/// Seeds a `str`/`bytes` the way `random.py` does for `version`: 1 folds the
/// code units with the `1000003` multiplier, 2 appends a SHA-512 digest and
/// reads the whole thing as a big-endian int; anything else hashes the object.
fn key_from_text(text: &SeedText<'_>, version: i64) -> Vec<u32> {
    match version {
        1 => {
            let units = text.units();
            let mut x: u64 = units.first().map_or(0, |first| first << 7);
            for unit in &units {
                x = x.wrapping_mul(1_000_003) ^ unit;
            }
            key_from_u64(x ^ units.len() as u64)
        }
        2 => {
            let mut data = text.bytes().to_vec();
            let digest = Sha512::digest(&data);
            data.extend_from_slice(&digest);
            key_from_biguint(&BigUint::from_bytes_be(&data))
        }
        _ => key_from_u64(text.hash()),
    }
}

/// Words of `n`'s absolute value, least significant first — the key
/// `random_seed` builds for an int.
fn key_from_bigint(n: &BigInt) -> Vec<u32> {
    key_from_biguint(n.magnitude())
}

/// Words of `n`, least significant first; zero is the single word `[0]`.
fn key_from_biguint(n: &BigUint) -> Vec<u32> {
    let words = n.to_u32_digits();
    if words.is_empty() { vec![0] } else { words }
}

/// Words of `n`, least significant first, as `random_seed` chunks an int.
fn key_from_u64(n: u64) -> Vec<u32> {
    #[expect(clippy::cast_possible_truncation, reason = "each half is masked to 32 bits")]
    let (lo, hi) = (n as u32, (n >> 32) as u32);
    if hi == 0 { vec![lo] } else { vec![lo, hi] }
}

/// CPython's `_Py_HashDouble`: the modulus-`2**61 - 1` hash `seed(float)` goes
/// through. Monty's own float hash (`Value::py_hash`) is not CPython's, so it
/// cannot be reused here without breaking seed parity. NaN hashes by object
/// identity in CPython, which has no reproducible answer; it reads as 0.
#[expect(clippy::many_single_char_names, reason = "CPython's names for the same algorithm")]
fn cpython_float_hash(v: f64) -> u64 {
    const BITS: u32 = 61;
    const MODULUS: u64 = (1 << BITS) - 1;
    const INF: i64 = 314_159;

    if v.is_nan() {
        return 0;
    }
    if v.is_infinite() {
        return (if v > 0.0 { INF } else { -INF }).cast_unsigned();
    }
    let (mut m, mut e) = libm::frexp(v);
    let sign: i64 = if m < 0.0 {
        m = -m;
        -1
    } else {
        1
    };
    let mut x: u64 = 0;
    while m != 0.0 {
        x = ((x << 28) & MODULUS) | x >> (BITS - 28);
        m *= 268_435_456.0;
        e -= 28;
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "m < 2**28 here")]
        let y = m as u64;
        m -= y as f64;
        x += y;
        if x >= MODULUS {
            x -= MODULUS;
        }
    }
    let bits = i32::try_from(BITS).expect("61 fits i32");
    let e = if e >= 0 { e % bits } else { bits - 1 - ((-1 - e) % bits) };
    let e = u32::try_from(e).expect("reduced exponent is non-negative");
    x = ((x << e) & MODULUS) | x >> (BITS - e);
    // `x` is below 2**61, so the signed product cannot overflow.
    let x = i64::try_from(x).expect("hash below 2**61") * sign;
    (if x == -1 { -2 } else { x }).cast_unsigned()
}

// ============================================================================
// MT19937
// ============================================================================

/// CPython's Mersenne Twister state, indexed exactly like `RandomObject`.
///
/// `state` always holds [`N`] words; it is a `Vec` only because serde cannot
/// derive a 624-element array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Mt19937 {
    state: Vec<u32>,
    index: usize,
}

impl Mt19937 {
    /// `init_by_array`: the state every seed shape reduces to. An empty key
    /// behaves as `[0]`, which is what CPython builds for `seed(0)`.
    pub(crate) fn from_key(key: &[u32]) -> Self {
        if key.is_empty() {
            return Self::from_key(&[0]);
        }
        let mut mt = Self::init_genrand(19_650_218);
        let s = &mut mt.state;
        let (mut i, mut j) = (1usize, 0usize);
        for _ in 0..N.max(key.len()) {
            #[expect(clippy::cast_possible_truncation, reason = "j < key.len(), far below 2**32")]
            let j32 = j as u32;
            s[i] = (s[i] ^ (s[i - 1] ^ (s[i - 1] >> 30)).wrapping_mul(1_664_525))
                .wrapping_add(key[j])
                .wrapping_add(j32);
            i += 1;
            j += 1;
            if i >= N {
                s[0] = s[N - 1];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..N - 1 {
            #[expect(clippy::cast_possible_truncation, reason = "i < N")]
            let i32 = i as u32;
            s[i] = (s[i] ^ (s[i - 1] ^ (s[i - 1] >> 30)).wrapping_mul(1_566_083_941)).wrapping_sub(i32);
            i += 1;
            if i >= N {
                s[0] = s[N - 1];
                i = 1;
            }
        }
        s[0] = 0x8000_0000;
        mt
    }

    /// Seeds from the OS entropy source, as `random_seed_urandom` does.
    pub(crate) fn from_os_entropy() -> Self {
        let mut bytes = [0u8; SEED_BYTES];
        getrandom::fill(&mut bytes).expect("OS entropy source unavailable");
        Self::from_entropy(&bytes)
    }

    /// Seeds from `SEED_BYTES` bytes of entropy read as little-endian words.
    pub(crate) fn from_entropy(bytes: &[u8]) -> Self {
        let words: Vec<u32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| u32::from_le_bytes(*chunk))
            .collect();
        Self::from_key(&words)
    }

    /// `init_genrand`: the linear seeding `init_by_array` starts from.
    fn init_genrand(seed: u32) -> Self {
        let mut state = vec![0u32; N];
        state[0] = seed;
        for i in 1..N {
            #[expect(clippy::cast_possible_truncation, reason = "i < N")]
            let i32 = i as u32;
            state[i] = 1_812_433_253u32
                .wrapping_mul(state[i - 1] ^ (state[i - 1] >> 30))
                .wrapping_add(i32);
        }
        Self { state, index: N }
    }

    /// `genrand_uint32`: the next tempered 32-bit output.
    pub(crate) fn next_u32(&mut self) -> u32 {
        if self.index >= N {
            self.twist();
        }
        let mut y = self.state[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    /// Regenerates all [`N`] words once the vector is exhausted.
    fn twist(&mut self) {
        fn mag(y: u32) -> u32 {
            if y & 1 == 0 { 0 } else { MATRIX_A }
        }
        let mt = &mut self.state;
        for kk in 0..N - M {
            let y = (mt[kk] & UPPER_MASK) | (mt[kk + 1] & LOWER_MASK);
            mt[kk] = mt[kk + M] ^ (y >> 1) ^ mag(y);
        }
        for kk in N - M..N - 1 {
            let y = (mt[kk] & UPPER_MASK) | (mt[kk + 1] & LOWER_MASK);
            mt[kk] = mt[kk + M - N] ^ (y >> 1) ^ mag(y);
        }
        let y = (mt[N - 1] & UPPER_MASK) | (mt[0] & LOWER_MASK);
        mt[N - 1] = mt[M - 1] ^ (y >> 1) ^ mag(y);
        self.index = 0;
    }

    /// `random()`: a float in `[0, 1)` with 53 random bits, built from two
    /// outputs exactly as `genrand_res53` does.
    pub(crate) fn random(&mut self) -> f64 {
        let a = self.next_u32() >> 5;
        let b = self.next_u32() >> 6;
        (f64::from(a) * 67_108_864.0 + f64::from(b)) * (1.0 / 9_007_199_254_740_992.0)
    }

    /// `getrandbits(k)` for `0 < k <= 128`, packed like the general case.
    pub(crate) fn getrandbits_u128(&mut self, k: u32) -> u128 {
        let mut result: u128 = 0;
        let mut remaining = k;
        let mut shift = 0;
        while remaining > 0 {
            let mut word = self.next_u32();
            if remaining < 32 {
                word >>= 32 - remaining;
            }
            result |= u128::from(word) << shift;
            shift += 32;
            remaining = remaining.saturating_sub(32);
        }
        result
    }

    /// `getrandbits(k)` for any `k > 0`, as little-endian 32-bit words: each
    /// word is one output, the last one shifted down to its remaining bits.
    /// Polls the deadline, since `k` is caller-chosen.
    pub(crate) fn getrandbits_words(&mut self, k: u64, words: usize, tracker: &ResourceTracker) -> RunResult<Vec<u32>> {
        let mut out = Vec::with_capacity(words);
        let mut remaining = k;
        for i in 0..words {
            tracker.check_time_every(i)?;
            let mut word = self.next_u32();
            if remaining < 32 {
                #[expect(clippy::cast_possible_truncation, reason = "remaining < 32")]
                let drop = 32 - remaining as u32;
                word >>= drop;
            }
            out.push(word);
            remaining = remaining.saturating_sub(32);
        }
        Ok(out)
    }

    /// `_randbelow_with_getrandbits(n)`: an int in `[0, n)` for `n > 0`, by
    /// rejection sampling `bit_length(n)` bits at a time.
    pub(crate) fn randbelow(&mut self, n: u128, tracker: &ResourceTracker) -> RunResult<u128> {
        debug_assert!(n > 0, "randbelow(0) is an empty range");
        let k = 128 - n.leading_zeros();
        let mut attempts = 0usize;
        loop {
            tracker.check_time_every(attempts)?;
            attempts = attempts.wrapping_add(1);
            let r = self.getrandbits_u128(k);
            if r < n {
                return Ok(r);
            }
        }
    }

    /// The state words and index `getstate()` reports.
    pub(crate) fn state(&self) -> (&[u32], usize) {
        (&self.state, self.index)
    }

    /// Restores a state `setstate()` validated: [`N`] words and an index in
    /// `0..=N`.
    pub(crate) fn from_state(state: Vec<u32>, index: usize) -> Self {
        debug_assert_eq!(state.len(), N);
        debug_assert!(index <= N);
        Self { state, index }
    }

    /// Words in the state vector, for `setstate()`'s size check.
    pub(crate) const fn state_len() -> usize {
        N
    }
}

// ============================================================================
// Heap object glue
// ============================================================================

impl HeapItem for Random {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // A generator owns no heap references.
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, Random> {
    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::Random
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    /// Generators hash by identity, as any class without `__eq__` does.
    fn py_hash(&self, _: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    fn py_repr_fmt(&self, f: &mut impl Write, _vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        Ok(write!(
            f,
            "<random.Random object at 0x{:x}>",
            self.py_identity().encoded()
        )?)
    }

    /// `VERSION`, the state-format number `getstate()` reports.
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        Ok((attr.static_string(vm.interns) == Some(StaticStrings::RandomVersion))
            .then_some(CallResult::Value(Value::Int(3))))
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        if let Some(function) = attr
            .static_string(vm.interns)
            .and_then(RandomFunctions::from_static_string)
        {
            random_dispatch(RandomTarget::Instance(self.id()), function, args, vm)
        } else {
            args.drop_with(vm);
            Err(ExcType::attribute_error("Random", attr.as_str(vm.interns)))
        }
    }
}

impl<'h> HeapRead<'h, Random> {
    /// Allocates a generator at the same point in the same sequence.
    ///
    /// How `copy.copy` rebuilds a generator: CPython pickles `getstate()` into
    /// a fresh `Random`, so the two draw the same numbers from then on. An
    /// unseeded generator copies as unseeded, and the two then take *separate*
    /// entropy from the host — CPython seeds at construction, so its copies
    /// agree (see `limitations/copy.md`).
    pub(crate) fn allocate_like(&self, vm: &mut VM<'h>) -> Value {
        let source = self.get(vm.heap);
        let copy = Random {
            rng: source.rng.clone(),
            gauss_next: source.gauss_next,
        };
        Value::Ref(vm.heap.allocate(HeapData::Random(Box::new(copy))))
    }
}

impl<'h> PyDeepCopy<'h> for HeapRead<'h, Random> {
    /// A generator holds no Python values, so its deep copy is the shallow one.
    #[inline(never)]
    fn py_deep_copy(&self, _source: &Value, _memo: &mut Memo, vm: &mut VM<'h>) -> RunResult<Value> {
        Ok(self.allocate_like(vm))
    }
}
