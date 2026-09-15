//! Implementation of Python's `random` module.
//!
//! The pure-Python bodies of `Lib/random.py` are ported here 1:1 on top of the
//! [`Mt19937`] core in `types::random`, so a seeded generator produces exactly
//! CPython's sequence: `randrange`, `choice`, `shuffle`, `sample`, `choices`
//! and the distributions all consume the same draws in the same order. The
//! module-level functions and the `random.Random` methods share one
//! dispatcher, [`random_dispatch`], parameterised by the [`RandomTarget`].
//!
//! Entropy is the host's: a generator nobody seeded suspends with an
//! `os.urandom` call the first time it draws, and the resume
//! ([`apply_seed_random`]) seeds it and re-runs the draw. See
//! `limitations/random.md` for the divergences.

use std::{
    cmp::Ordering,
    f64::consts::{E, PI, TAU},
    iter::once,
    mem,
};

use ahash::AHashSet;
use monty_types::{OsFunctionCall, UrandomArgs};
use num_bigint::{BigInt, BigUint};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::{
    args::{ArgValues, FromArgs},
    builtins::Builtins,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    heap::{ContainsHeap, DropWithContext, HeapData, HeapId, HeapReadOutput},
    intern::StaticStrings,
    modules::ModuleFunctions,
    os_dispatch::PostConversionEffect,
    types::{
        List, LongInt, Module, PyTrait, Type,
        bytes::allocate_bytes,
        iter::collect_owned_iterable,
        long_int::bigint_to_f64_checked,
        random::{Mt19937, RandomTarget, SEED_BYTES, seed_key_from_value},
        tuple::{TupleVec, allocate_tuple},
    },
    value::{VALUE_SIZE, Value},
};

/// `random` module functions, each also a method of `random.Random`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, Serialize, Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum RandomFunctions {
    Random,
    Seed,
    Getstate,
    Setstate,
    Getrandbits,
    Randbytes,
    Randrange,
    Randint,
    Choice,
    Choices,
    Shuffle,
    Sample,
    Uniform,
    Triangular,
    Normalvariate,
    Gauss,
    Lognormvariate,
    Expovariate,
    Vonmisesvariate,
    Gammavariate,
    Betavariate,
    Paretovariate,
    Weibullvariate,
    Binomialvariate,
}

/// Every function with the interned name it is bound to, for the module
/// namespace and for method lookup on `random.Random`.
const RANDOM_FUNCTIONS: &[(StaticStrings, RandomFunctions)] = &[
    (StaticStrings::Random, RandomFunctions::Random),
    (StaticStrings::Seed, RandomFunctions::Seed),
    (StaticStrings::Getstate, RandomFunctions::Getstate),
    (StaticStrings::Setstate, RandomFunctions::Setstate),
    (StaticStrings::Getrandbits, RandomFunctions::Getrandbits),
    (StaticStrings::Randbytes, RandomFunctions::Randbytes),
    (StaticStrings::Randrange, RandomFunctions::Randrange),
    (StaticStrings::Randint, RandomFunctions::Randint),
    (StaticStrings::Choice, RandomFunctions::Choice),
    (StaticStrings::Choices, RandomFunctions::Choices),
    (StaticStrings::Shuffle, RandomFunctions::Shuffle),
    (StaticStrings::Sample, RandomFunctions::Sample),
    (StaticStrings::Uniform, RandomFunctions::Uniform),
    (StaticStrings::Triangular, RandomFunctions::Triangular),
    (StaticStrings::Normalvariate, RandomFunctions::Normalvariate),
    (StaticStrings::Gauss, RandomFunctions::Gauss),
    (StaticStrings::Lognormvariate, RandomFunctions::Lognormvariate),
    (StaticStrings::Expovariate, RandomFunctions::Expovariate),
    (StaticStrings::Vonmisesvariate, RandomFunctions::Vonmisesvariate),
    (StaticStrings::Gammavariate, RandomFunctions::Gammavariate),
    (StaticStrings::Betavariate, RandomFunctions::Betavariate),
    (StaticStrings::Paretovariate, RandomFunctions::Paretovariate),
    (StaticStrings::Weibullvariate, RandomFunctions::Weibullvariate),
    (StaticStrings::Binomialvariate, RandomFunctions::Binomialvariate),
];

impl RandomFunctions {
    /// The function a `random.Random` attribute name refers to, if any.
    pub(crate) fn from_static_string(name: StaticStrings) -> Option<Self> {
        RANDOM_FUNCTIONS
            .iter()
            .find_map(|(attr, function)| (*attr == name).then_some(*function))
    }
}

/// Creates the `random` module on the heap.
///
/// # Panics
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Random);
    for (name, function) in RANDOM_FUNCTIONS {
        module.set_attr(*name, Value::ModuleFunction(ModuleFunctions::Random(*function)), vm);
    }
    module.set_attr(
        StaticStrings::RandomClass,
        Value::Builtin(Builtins::Type(Type::Random)),
        vm,
    );
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a module-level call, which acts on the VM's own generator.
pub(super) fn call(vm: &mut VM<'_>, function: RandomFunctions, args: ArgValues) -> RunResult<CallResult> {
    random_dispatch(RandomTarget::Global, function, args, vm)
}

/// Runs `function` against `target`'s generator, for module functions and
/// `Random` methods alike.
///
/// A draw from an unseeded generator suspends for host entropy instead,
/// stashing the call in a [`RandomRetry`] that the resume replays once the
/// generator is seeded. Only `seed(x)` and `setstate()` never need entropy.
pub(crate) fn random_dispatch(
    target: RandomTarget,
    function: RandomFunctions,
    args: ArgValues,
    vm: &mut VM<'_>,
) -> RunResult<CallResult> {
    match function {
        RandomFunctions::Seed => seed(target, args, vm),
        RandomFunctions::Setstate => setstate(target, args, vm).map(CallResult::Value),
        _ if !target.is_seeded(vm) => Ok(request_entropy(target, Some(RandomRetry { function, args }), vm)),
        _ => call_seeded(target, function, args, vm).map(CallResult::Value),
    }
}

/// A draw stashed while its generator waits for host entropy, replayed by
/// [`apply_seed_random`]. Owns the call's arguments across the yield.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct RandomRetry {
    function: RandomFunctions,
    args: ArgValues,
}

impl<C: ContainsHeap> DropWithContext<C> for RandomRetry {
    fn drop_with(self, ctx: &mut C) {
        self.args.drop_with(ctx);
    }
}

/// Suspends for [`SEED_BYTES`] of host entropy, pinning an instance target
/// across the yield (released by [`apply_seed_random`] or the effect's `release`).
fn request_entropy(target: RandomTarget, retry: Option<RandomRetry>, vm: &mut VM<'_>) -> CallResult {
    if let RandomTarget::Instance(id) = target {
        vm.heap.inc_ref(id);
    }
    CallResult::OsCallWithEffect {
        call: OsFunctionCall::Urandom(UrandomArgs {
            size: i64::try_from(SEED_BYTES).expect("seed size fits i64"),
        }),
        effect: PostConversionEffect::SeedRandom { target, retry }.into(),
    }
}

/// Resume half of [`request_entropy`]: seeds `target` from the host's
/// `os.urandom` reply, then answers `None` (`seed()`) or replays the stashed
/// draw. The instance pin is dropped on every path.
pub(crate) fn apply_seed_random(
    target: RandomTarget,
    retry: Option<RandomRetry>,
    reply: Value,
    vm: &mut VM<'_>,
) -> RunResult<Value> {
    let result = seed_from_reply(target, retry, reply, vm);
    if let RandomTarget::Instance(id) = target {
        vm.heap.dec_ref(id);
    }
    result
}

/// [`apply_seed_random`] without the unpin: validates the reply as exactly
/// [`SEED_BYTES`] bytes, seeds, and replays.
fn seed_from_reply(
    target: RandomTarget,
    retry: Option<RandomRetry>,
    reply: Value,
    vm: &mut VM<'_>,
) -> RunResult<Value> {
    defer_drop!(reply, vm);
    let seeded = match value_as_bytes(reply, vm) {
        Some(bytes) if bytes.len() == SEED_BYTES => Ok(Mt19937::from_entropy(bytes)),
        Some(bytes) => Err(format!(
            "'os.urandom' returned {} bytes, expected {SEED_BYTES}",
            bytes.len()
        )),
        None => Err(format!(
            "'os.urandom' must return bytes, not {}",
            reply.py_type_name(vm)
        )),
    };
    let rng = match seeded {
        Ok(rng) => rng,
        Err(message) => {
            retry.drop_with(vm);
            return Err(SimpleException::new_msg(ExcType::RuntimeError, message).into());
        }
    };
    target.reseed(vm, rng);
    match retry {
        None => Ok(Value::None),
        Some(RandomRetry { function, args }) => match random_dispatch(target, function, args, vm)? {
            CallResult::Value(value) => Ok(value),
            other => {
                other.drop_with(vm);
                unreachable!("a seeded generator never suspends")
            }
        },
    }
}

/// Borrows the bytes behind a `bytes` value, interned or heap-allocated.
fn value_as_bytes<'a>(value: &'a Value, vm: &'a VM<'_>) -> Option<&'a [u8]> {
    match value {
        Value::InternBytes(id) => Some(vm.interns.get_bytes(*id)),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Bytes(bytes) => Some(bytes.as_slice()),
            _ => None,
        },
        _ => None,
    }
}

/// Dispatch for a generator known to be seeded.
fn call_seeded(target: RandomTarget, function: RandomFunctions, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    match function {
        RandomFunctions::Random => random(target, args, vm),
        RandomFunctions::Getstate => getstate(target, args, vm),
        RandomFunctions::Getrandbits => getrandbits(target, args, vm),
        RandomFunctions::Randbytes => randbytes(target, args, vm),
        RandomFunctions::Randrange => randrange(target, args, vm),
        RandomFunctions::Randint => randint(target, args, vm),
        RandomFunctions::Choice => choice(target, args, vm),
        RandomFunctions::Choices => choices(target, args, vm),
        RandomFunctions::Shuffle => shuffle(target, args, vm),
        RandomFunctions::Sample => sample(target, args, vm),
        RandomFunctions::Uniform => uniform(target, args, vm),
        RandomFunctions::Triangular => triangular(target, args, vm),
        RandomFunctions::Normalvariate => normalvariate(target, args, vm),
        RandomFunctions::Gauss => gauss(target, args, vm),
        RandomFunctions::Lognormvariate => lognormvariate(target, args, vm),
        RandomFunctions::Expovariate => expovariate(target, args, vm),
        RandomFunctions::Vonmisesvariate => vonmisesvariate(target, args, vm),
        RandomFunctions::Gammavariate => gammavariate(target, args, vm),
        RandomFunctions::Betavariate => betavariate(target, args, vm),
        RandomFunctions::Paretovariate => paretovariate(target, args, vm),
        RandomFunctions::Weibullvariate => weibullvariate(target, args, vm),
        RandomFunctions::Binomialvariate => binomialvariate(target, args, vm),
        RandomFunctions::Seed | RandomFunctions::Setstate => {
            unreachable!("seed and setstate are dispatched before the seeded check")
        }
    }
}

// ============================================================================
// Seeding and state
// ============================================================================

/// `seed(a=None, version=2)` — a Python `def` in CPython.
#[derive(FromArgs)]
#[from_args(name = "Random.seed", style = def)]
struct SeedArgs {
    #[from_args(default = Value::None)]
    a: Value,
    #[from_args(default = Value::Int(2))]
    version: Value,
}

/// `seed(a=None, version=2)`: `None` asks the host for entropy (and answers
/// `None` on resume); anything else seeds synchronously.
fn seed(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<CallResult> {
    let SeedArgs { a, version } = SeedArgs::from_args(args, vm)?;
    defer_drop!(a, vm);
    defer_drop!(version, vm);
    if matches!(a, Value::None) {
        return Ok(request_entropy(target, None, vm));
    }
    let key = seed_key_from_value(a, version_number(version), vm)?;
    target.reseed(vm, Mt19937::from_key(&key));
    Ok(CallResult::Value(Value::None))
}

/// The `version` a seed call named, for the `== 1` / `== 2` tests in
/// `random.py`; anything that equals neither reads as 0.
fn version_number(version: &Value) -> i64 {
    match version {
        Value::Int(i) => *i,
        Value::Bool(b) => i64::from(*b),
        #[expect(clippy::cast_possible_truncation, reason = "only 1.0 and 2.0 matter")]
        Value::Float(f) if f.fract() == 0.0 && f.abs() < 3.0 => *f as i64,
        _ => 0,
    }
}

/// `getstate()`: `(3, (624 words..., index), gauss_next)`.
fn getstate(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    args.check_zero_args("Random.getstate", vm.heap)?;
    let (words, index, gauss_next) = target.with_generator(vm, |random, _| {
        let (words, index) = random.rng().state();
        (words.to_vec(), index, random.gauss_next())
    });
    let internal: TupleVec = words
        .into_iter()
        .map(|word| Value::Int(i64::from(word)))
        .chain(once(Value::Int(i64::try_from(index).expect("index fits i64"))))
        .collect();
    let internal = allocate_tuple(internal, vm.heap);
    let gauss_next = gauss_next.map_or(Value::None, Value::Float);
    Ok(allocate_tuple(
        SmallVec::from_vec(vec![Value::Int(3), internal, gauss_next]),
        vm.heap,
    ))
}

/// `setstate(state)` — a Python `def` wrapping the C `setstate`.
#[derive(FromArgs)]
#[from_args(name = "Random.setstate", style = def)]
struct SetstateArgs {
    state: Value,
}

/// `setstate(state)`: restores a `getstate()` tuple, accepting version 2
/// states (signed words) as CPython does.
fn setstate(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let SetstateArgs { state } = SetstateArgs::from_args(args, vm)?;
    defer_drop!(state, vm);

    // `version = state[0]` runs before the unpack, so a non-subscriptable
    // state reports that rather than an unpack failure.
    let version = state.py_getitem(&Value::Int(0), vm)?;
    defer_drop!(version, vm);
    let version = match version {
        Value::Int(v @ (2 | 3)) => *v,
        // `version == 3` in Python, which `3.0` satisfies.
        Value::Float(f) if f.fract() == 0.0 && (2.0..=3.0).contains(f) => {
            #[expect(clippy::cast_possible_truncation, reason = "exactly 2.0 or 3.0")]
            let v = *f as i64;
            v
        }
        other => {
            let text = other.py_str(vm)?;
            defer_drop!(text, vm);
            let text = text.to_str_heap(vm.heap, vm.interns)?;
            return Err(ExcType::value_error(format!(
                "state with version {text} passed to Random.setstate() of version 3"
            )));
        }
    };

    let items: Vec<Value> = collect_owned_iterable(state.clone_with_heap(vm), vm)?;
    defer_drop!(items, vm);
    match items.len().cmp(&3) {
        Ordering::Less => {
            return Err(ExcType::value_error(format!(
                "not enough values to unpack (expected 3, got {})",
                items.len()
            )));
        }
        Ordering::Greater => {
            return Err(ExcType::value_error(format!(
                "too many values to unpack (expected 3, got {})",
                items.len()
            )));
        }
        Ordering::Equal => {}
    }
    let internal = &items[1];
    let gauss_next = match &items[2] {
        Value::None => None,
        Value::Float(f) => Some(*f),
        Value::Int(i) => Some(*i as f64),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        other => {
            return Err(ExcType::type_error(format!(
                "Random.setstate() gauss_next must be a float or None, not {}",
                other.py_type_name(vm)
            )));
        }
    };

    let Value::Ref(internal_id) = internal else {
        return Err(ExcType::type_error("state vector must be a tuple"));
    };
    let HeapData::Tuple(tuple) = vm.heap.get(*internal_id) else {
        return Err(ExcType::type_error("state vector must be a tuple"));
    };
    let entries = tuple.as_slice();
    if entries.len() != Mt19937::state_len() + 1 {
        return Err(ExcType::value_error("state vector is the wrong size"));
    }
    let mut words = Vec::with_capacity(Mt19937::state_len());
    for entry in &entries[..Mt19937::state_len()] {
        words.push(state_word(entry, version, vm)?);
    }
    let index = match entries[Mt19937::state_len()] {
        Value::Int(i) => i,
        Value::Bool(b) => i64::from(b),
        ref other => {
            return Err(ExcType::type_error(format!(
                "'{}' object cannot be interpreted as an integer",
                other.py_type_name(vm)
            )));
        }
    };
    let index = usize::try_from(index)
        .ok()
        .filter(|index| *index <= Mt19937::state_len())
        .ok_or_else(|| ExcType::value_error("invalid state"))?;

    target.with_generator(vm, |random, _| {
        random.reseed(Mt19937::from_state(words, index));
        random.set_gauss_next(gauss_next);
    });
    Ok(Value::None)
}

/// One state word as the C `setstate` reads it (`PyLong_AsUnsignedLong`,
/// truncated to 32 bits), after `random.py` has reduced a version 2 word
/// modulo `2**32`.
fn state_word(entry: &Value, version: i64, vm: &VM<'_>) -> RunResult<u32> {
    const NEGATIVE: &str = "can't convert negative value to unsigned int";
    const TOO_LARGE: &str = "Python int too large to convert to C unsigned long";
    let overflow = |message: &str| SimpleException::new_msg(ExcType::OverflowError, message).into();
    match entry {
        Value::Bool(b) => Ok(u32::from(*b)),
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "truncation to 32 bits is the C behaviour; the sign is checked first"
        )]
        Value::Int(i) => {
            if version == 2 {
                Ok(i.rem_euclid(1 << 32) as u32)
            } else if *i < 0 {
                Err(overflow(NEGATIVE))
            } else {
                Ok(*i as u32)
            }
        }
        _ => match entry.as_long_int(vm) {
            Some(big) if version == 2 => Ok(low_word(big)),
            Some(big) if big.sign() == num_bigint::Sign::Minus => Err(overflow(NEGATIVE)),
            Some(_) => Err(overflow(TOO_LARGE)),
            None => Err(ExcType::type_error("an integer is required")),
        },
    }
}

/// `big % 2**32` as a word, for version 2 state vectors holding big ints.
fn low_word(big: &BigInt) -> u32 {
    let modulus = BigInt::from(1u64 << 32);
    let reduced = ((big % &modulus) + &modulus) % &modulus;
    reduced.to_u32_digits().1.first().copied().unwrap_or(0)
}

// ============================================================================
// Core draws
// ============================================================================

/// `random()`: the next float in `[0, 1)`.
fn random(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    args.check_zero_args("Random.random", vm.heap)?;
    Ok(Value::Float(
        target.with_generator(vm, |random, _| random.rng().random()),
    ))
}

/// `getrandbits(k)`: `k` random bits as a non-negative int.
fn getrandbits(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let k = args.get_one_arg("Random.getrandbits", vm.heap)?;
    defer_drop!(k, vm);
    let k = k.as_int(vm)?;
    let k = u64::try_from(k).map_err(|_| ExcType::value_error("Cannot convert negative int"))?;
    if k == 0 {
        return Ok(Value::Int(0));
    }
    if k <= 64 {
        #[expect(clippy::cast_possible_truncation, reason = "k <= 64")]
        let bits = target.with_generator(vm, |random, _| random.rng().getrandbits_u128(k as u32));
        return Ok(LongInt::value_from_u128(bits, vm.heap));
    }
    let words = usize::try_from((k - 1) / 32 + 1).map_err(|_| ExcType::value_error("number of bits too large"))?;
    // One preflight for the word buffer and the big int built from it.
    vm.heap.tracker.check_allocation(words.saturating_mul(8))?;
    let words = target.with_generator(vm, |random, _| random.rng().getrandbits_words(k, words));
    Ok(LongInt::new(BigInt::from(BigUint::from_slice(&words))).into_value(vm.heap))
}

/// `randbytes(n)` — a Python `def`: `getrandbits(n * 8).to_bytes(n, 'little')`.
#[derive(FromArgs)]
#[from_args(name = "Random.randbytes", style = def)]
struct RandbytesArgs {
    n: Value,
}

/// `randbytes(n)`: `n` random bytes, the little-endian bytes of `getrandbits(8n)`.
fn randbytes(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let RandbytesArgs { n } = RandbytesArgs::from_args(args, vm)?;
    defer_drop!(n, vm);
    let n = n.as_int(vm)?;
    // `getrandbits(n * 8)` is what rejects a negative count.
    let n = usize::try_from(n).map_err(|_| ExcType::value_error("Cannot convert negative int"))?;
    if n == 0 {
        return Ok(allocate_bytes(Vec::new(), vm.heap));
    }
    vm.heap.tracker.check_allocation(n.saturating_mul(2))?;
    let words = n.div_ceil(4);
    let bits = u64::try_from(n).expect("usize fits u64") * 8;
    let words = target.with_generator(vm, |random, _| random.rng().getrandbits_words(bits, words));
    let mut bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    bytes.truncate(n);
    Ok(allocate_bytes(bytes, vm.heap))
}

/// `randrange(start, stop=None, step=1)` — a Python `def`. `step` is optional
/// rather than defaulted so the `step is not _ONE` check can tell an explicit
/// step from the default.
#[derive(FromArgs)]
#[from_args(name = "Random.randrange", style = def)]
struct RandrangeArgs {
    start: Value,
    #[from_args(default = Value::None)]
    stop: Value,
    #[from_args(default)]
    step: Option<Value>,
}

/// `randrange(start, stop=None, step=1)`: a random member of the range.
fn randrange(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let RandrangeArgs { start, stop, step } = RandrangeArgs::from_args(args, vm)?;
    defer_drop!(start, vm);
    defer_drop!(stop, vm);
    defer_drop!(step, vm);

    let istart = i128::from(start.as_int(vm)?);
    if matches!(stop, Value::None) {
        // CPython tests `step is not _ONE`, and `1` is that very object.
        if step.as_ref().is_some_and(|step| !matches!(step, Value::Int(1))) {
            return Err(ExcType::type_error("Missing a non-None stop argument"));
        }
        return if istart > 0 {
            Ok(int_value(istart_plus(
                0,
                target.with_generator(vm, |random, _| random.rng().randbelow(as_u128(istart))),
            )))
        } else {
            Err(ExcType::value_error("empty range for randrange()"))
        };
    }

    let istop = i128::from(stop.as_int(vm)?);
    let width = istop - istart;
    let istep = match step {
        Some(step) => i128::from(step.as_int(vm)?),
        None => 1,
    };
    if istep == 1 {
        return if width > 0 {
            let offset = target.with_generator(vm, |random, _| random.rng().randbelow(as_u128(width)));
            Ok(int_value(istart_plus(istart, offset)))
        } else {
            Err(ExcType::value_error(format!(
                "empty range in randrange({}, {})",
                int_arg_str(start),
                int_arg_str(stop)
            )))
        };
    }

    let n = match istep.cmp(&0) {
        Ordering::Greater => floor_div(width + istep - 1, istep),
        Ordering::Less => floor_div(width + istep + 1, istep),
        Ordering::Equal => return Err(ExcType::value_error("zero step for randrange()")),
    };
    if n <= 0 {
        let step = step.as_ref().expect("a unit step returned above");
        return Err(ExcType::value_error(format!(
            "empty range in randrange({}, {}, {})",
            int_arg_str(start),
            int_arg_str(stop),
            int_arg_str(step)
        )));
    }
    let offset = target.with_generator(vm, |random, _| random.rng().randbelow(as_u128(n)));
    Ok(int_value(
        istart + istep * i128::try_from(offset).expect("offset below n"),
    ))
}

/// `randint(a, b)` — a Python `def`.
#[derive(FromArgs)]
#[from_args(name = "Random.randint", style = def)]
struct RandintArgs {
    a: Value,
    b: Value,
}

/// `randint(a, b)`: a random int in `[a, b]`.
fn randint(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let RandintArgs { a, b } = RandintArgs::from_args(args, vm)?;
    defer_drop!(a, vm);
    defer_drop!(b, vm);
    let a = i128::from(a.as_int(vm)?);
    let b = i128::from(b.as_int(vm)?);
    if b < a {
        return Err(ExcType::value_error(format!("empty range in randint({a}, {b})")));
    }
    let offset = target.with_generator(vm, |random, _| random.rng().randbelow(as_u128(b - a + 1)));
    Ok(int_value(istart_plus(a, offset)))
}

/// `start + offset` for a `randbelow` draw that cannot leave the range.
fn istart_plus(start: i128, offset: u128) -> i128 {
    start + i128::try_from(offset).expect("offset below an i64-sized range")
}

/// A positive range width as the unsigned count `randbelow` takes.
fn as_u128(n: i128) -> u128 {
    u128::try_from(n).expect("range width checked positive")
}

/// An int known to sit inside an `i64` range, back to a `Value`.
fn int_value(n: i128) -> Value {
    Value::Int(i64::try_from(n).expect("result lies within the i64 arguments"))
}

/// Python's floor division for the `randrange` step arithmetic.
fn floor_div(a: i128, b: i128) -> i128 {
    let q = a / b;
    if (a % b != 0) && ((a < 0) != (b < 0)) { q - 1 } else { q }
}

/// How `randrange` spells an argument in its empty-range message: the
/// f-string formats the original object, so `True` stays `True`.
fn int_arg_str(value: &Value) -> String {
    match value {
        Value::Bool(b) => if *b { "True" } else { "False" }.to_owned(),
        Value::Int(i) => i.to_string(),
        _ => unreachable!("only ints and bools pass `as_int`"),
    }
}

// ============================================================================
// Sequences
// ============================================================================

/// `choice(seq)` — a Python `def`.
#[derive(FromArgs)]
#[from_args(name = "Random.choice", style = def)]
struct ChoiceArgs {
    seq: Value,
}

/// `choice(seq)`: `seq[randbelow(len(seq))]`.
fn choice(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let ChoiceArgs { seq } = ChoiceArgs::from_args(args, vm)?;
    defer_drop!(seq, vm);
    let len = len_of(seq, vm)?;
    if len == 0 {
        return Err(ExcType::index_error("Cannot choose from an empty sequence"));
    }
    let index = target.with_generator(vm, |random, _| random.rng().randbelow(len as u128));
    seq.py_getitem(&Value::Int(index_value(index)), vm)
}

/// `shuffle(x)` — a Python `def`.
#[derive(FromArgs)]
#[from_args(name = "Random.shuffle", style = def)]
struct ShuffleArgs {
    x: Value,
}

/// `shuffle(x)`: Fisher–Yates over a list in place, from the end.
///
/// Only lists can be shuffled: any other sequence of two or more items raises
/// the item-assignment `TypeError` CPython's first swap would — after the
/// draw that swap was going to use, so the stream stays aligned.
fn shuffle(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let ShuffleArgs { x } = ShuffleArgs::from_args(args, vm)?;
    defer_drop!(x, vm);
    let len = len_of(x, vm)?;
    let list_id = match x {
        Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::List(_)) => *id,
        _ if len < 2 => return Ok(Value::None),
        other => {
            target.with_generator(vm, |random, _| random.rng().randbelow(len as u128));
            return Err(ExcType::type_error_not_sub_assignment(&other.py_type_name(vm)));
        }
    };
    let HeapReadOutput::List(mut list) = vm.heap.read(list_id) else {
        unreachable!("checked to be a list above");
    };
    target.with_generator(vm, |random, vm| {
        for i in (1..len).rev() {
            vm.heap.tracker.check_time_every(i)?;
            let j = index_value(random.rng().randbelow(i as u128 + 1));
            list.get_mut(vm.heap)
                .as_vec_mut()
                .swap(i, usize::try_from(j).expect("j <= i"));
        }
        Ok(Value::None)
    })
}

/// `sample(population, k, *, counts=None)` — a Python `def`.
#[derive(FromArgs)]
#[from_args(name = "Random.sample", style = def)]
struct SampleArgs {
    population: Value,
    k: Value,
    #[from_args(kw_only, default = Value::None)]
    counts: Value,
}

/// `sample(population, k, *, counts=None)`: `k` distinct picks, in selection
/// order, drawn exactly as CPython does (pool below its set-size threshold,
/// rejection into a set above it).
fn sample(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let SampleArgs { population, k, counts } = SampleArgs::from_args(args, vm)?;
    defer_drop!(population, vm);
    defer_drop!(k, vm);
    defer_drop!(counts, vm);

    if !is_sequence(population.py_type_heap(vm.heap)) {
        return Err(ExcType::type_error(
            "Population must be a sequence.  For dicts or sets, use sorted(d).",
        ));
    }
    let n = len_of(population, vm)?;
    let k = k.as_int(vm)?;

    let picks = if matches!(counts, Value::None) {
        let indices = sample_indices(target, n, k, vm)?;
        collect_items(population, indices.into_iter(), vm)?
    } else {
        let cum_counts = cumulative_counts(counts, vm)?;
        if cum_counts.len() != n {
            return Err(ExcType::value_error(
                "The number of counts does not match the population",
            ));
        }
        let total = cum_counts.last().copied().unwrap_or(0);
        if total < 0 {
            return Err(ExcType::value_error("Counts must be non-negative"));
        }
        let total = usize::try_from(total).expect("non-negative");
        let selections = sample_indices(target, total, k, vm)?;
        let indices = selections
            .into_iter()
            .map(|s| bisect_right(&cum_counts, i128::try_from(s).expect("selection below total")));
        collect_items(population, indices, vm)?
    };
    Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(picks)))))
}

/// The positions `sample` picks from a population of `n` — the whole
/// algorithm from `random.py`, so the draws match CPython's.
fn sample_indices(target: RandomTarget, n: usize, k: i64, vm: &mut VM<'_>) -> RunResult<Vec<usize>> {
    let k = match usize::try_from(k) {
        Ok(k) if k <= n => k,
        _ => return Err(ExcType::value_error("Sample larger than population or is negative")),
    };
    vm.heap.tracker.check_allocation(k.saturating_mul(VALUE_SIZE))?;

    // `setsize = 21 + 4 ** ceil(log(k * 3, 4))` for k > 5: the point where a
    // k-sized set costs less than an n-sized pool.
    let setsize: usize = if k > 5 {
        let exponent = ((k * 3) as f64).ln() / 4f64.ln();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "small positive exponent"
        )]
        let exponent = exponent.ceil() as u32;
        4usize.checked_pow(exponent).map_or(usize::MAX, |table| 21 + table)
    } else {
        21
    };

    target.with_generator(vm, |random, vm| {
        let rng = random.rng();
        let mut result = Vec::with_capacity(k);
        if n <= setsize {
            let mut pool: Vec<usize> = (0..n).collect();
            for i in 0..k {
                vm.heap.tracker.check_time_every(i)?;
                let j = index_value(rng.randbelow((n - i) as u128));
                let j = usize::try_from(j).expect("j < n");
                result.push(pool[j]);
                pool[j] = pool[n - i - 1];
            }
        } else {
            let mut selected = AHashSet::with_capacity(k);
            for i in 0..k {
                vm.heap.tracker.check_time_every(i)?;
                let mut j = index_value(rng.randbelow(n as u128));
                while selected.contains(&j) {
                    j = index_value(rng.randbelow(n as u128));
                }
                selected.insert(j);
                result.push(usize::try_from(j).expect("j < n"));
            }
        }
        Ok(result)
    })
}

/// `list(accumulate(counts))` for `sample(counts=...)`, which must be ints.
fn cumulative_counts(counts: &Value, vm: &mut VM<'_>) -> RunResult<Vec<i128>> {
    let items: Vec<Value> = collect_owned_iterable(counts.clone_with_heap(vm), vm)?;
    defer_drop!(items, vm);
    let mut cumulative = Vec::with_capacity(items.len());
    let mut running: i128 = 0;
    for item in items {
        let count = match item {
            Value::Int(i) => i128::from(*i),
            Value::Bool(b) => i128::from(*b),
            _ => return Err(ExcType::type_error("Counts must be integers")),
        };
        running += count;
        cumulative.push(running);
    }
    Ok(cumulative)
}

/// `choices(population, weights=None, *, cum_weights=None, k=1)` — a Python `def`.
#[derive(FromArgs)]
#[from_args(name = "Random.choices", style = def)]
struct ChoicesArgs {
    population: Value,
    #[from_args(default = Value::None)]
    weights: Value,
    #[from_args(kw_only, default = Value::None)]
    cum_weights: Value,
    #[from_args(kw_only, default = Value::Int(1))]
    k: Value,
}

/// `choices(population, weights=None, *, cum_weights=None, k=1)`: `k` picks
/// with replacement, uniform or by (cumulative) weight.
fn choices(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let ChoicesArgs {
        population,
        weights,
        cum_weights,
        k,
    } = ChoicesArgs::from_args(args, vm)?;
    defer_drop!(population, vm);
    defer_drop!(weights, vm);
    defer_drop!(cum_weights, vm);
    defer_drop!(k, vm);

    let n = len_of(population, vm)?;
    let cumulative = if matches!(cum_weights, Value::None) {
        if matches!(weights, Value::None) {
            None
        } else if let Value::Int(k) = weights {
            // The common mistake `choices(pop, 5)`, which CPython names too.
            return Err(ExcType::type_error(format!(
                "The number of choices must be a keyword argument: k={k}"
            )));
        } else {
            Some(cumulative_weights(weights, vm)?)
        }
    } else if matches!(weights, Value::None) {
        let items: Vec<Value> = collect_owned_iterable(cum_weights.clone_with_heap(vm), vm)?;
        defer_drop!(items, vm);
        let mut cumulative = Vec::with_capacity(items.len());
        for item in items {
            cumulative.push(to_float(item, vm)?);
        }
        Some(cumulative)
    } else {
        return Err(ExcType::type_error(
            "Cannot specify both weights and cumulative weights",
        ));
    };
    let k = k.as_int(vm)?;
    let k = usize::try_from(k).unwrap_or(0);
    vm.heap.tracker.check_allocation(k.saturating_mul(VALUE_SIZE))?;

    let indices: Vec<usize> = match cumulative {
        None => {
            // `floor(random() * n)` with `n` as a float, as CPython computes it.
            let n_f = n as f64;
            target.with_generator(vm, |random, _| {
                let rng = random.rng();
                #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "0 <= floor < n")]
                (0..k).map(|_| (rng.random() * n_f).floor() as usize).collect()
            })
        }
        Some(cumulative) => {
            if cumulative.len() != n {
                return Err(ExcType::value_error(
                    "The number of weights does not match the population",
                ));
            }
            let total = cumulative
                .last()
                .copied()
                .ok_or_else(|| ExcType::index_error("list index out of range"))?
                + 0.0;
            if total <= 0.0 {
                return Err(ExcType::value_error("Total of weights must be greater than zero"));
            }
            if !total.is_finite() {
                return Err(ExcType::value_error("Total of weights must be finite"));
            }
            let hi = n - 1;
            target.with_generator(vm, |random, _| {
                let rng = random.rng();
                (0..k)
                    .map(|_| bisect_right_f64(&cumulative, rng.random() * total, hi))
                    .collect()
            })
        }
    };
    let picks = collect_items(population, indices.into_iter(), vm)?;
    Ok(Value::Ref(vm.heap.allocate(HeapData::List(List::new(picks)))))
}

/// `list(accumulate(weights))` for `choices`, as floats.
fn cumulative_weights(weights: &Value, vm: &mut VM<'_>) -> RunResult<Vec<f64>> {
    let items: Vec<Value> = collect_owned_iterable(weights.clone_with_heap(vm), vm)?;
    defer_drop!(items, vm);
    let mut cumulative = Vec::with_capacity(items.len());
    let mut running = 0.0;
    for item in items {
        running += to_float(item, vm)?;
        cumulative.push(running);
    }
    Ok(cumulative)
}

/// `population[i]` for each index, as owned values for a new list.
fn collect_items(population: &Value, indices: impl Iterator<Item = usize>, vm: &mut VM<'_>) -> RunResult<Vec<Value>> {
    let mut items: Vec<Value> = Vec::new();
    for index in indices {
        match population.py_getitem(&Value::Int(i64::try_from(index).expect("index fits i64")), vm) {
            Ok(item) => items.push(item),
            Err(err) => {
                items.drop_with(vm);
                return Err(err);
            }
        }
    }
    Ok(items)
}

/// `bisect.bisect_right(a, x)` over cumulative counts.
fn bisect_right(a: &[i128], x: i128) -> usize {
    let (mut lo, mut hi) = (0, a.len());
    while lo < hi {
        let mid = lo.midpoint(hi);
        if x < a[mid] { hi = mid } else { lo = mid + 1 }
    }
    lo
}

/// `bisect.bisect_right(a, x, 0, hi)` over cumulative weights.
fn bisect_right_f64(a: &[f64], x: f64, hi: usize) -> usize {
    let (mut lo, mut hi) = (0, hi);
    while lo < hi {
        let mid = lo.midpoint(hi);
        if x < a[mid] { hi = mid } else { lo = mid + 1 }
    }
    lo
}

/// Whether `sample` accepts `ty` as a population (`collections.abc.Sequence`).
fn is_sequence(ty: Type) -> bool {
    matches!(
        ty,
        Type::List | Type::Tuple | Type::Str | Type::Bytes | Type::Range | Type::Deque
    )
}

/// `len(value)`, with `len()`'s own `TypeError` for unsized values.
fn len_of(value: &Value, vm: &VM<'_>) -> RunResult<usize> {
    value
        .py_len(vm)
        .ok_or_else(|| ExcType::type_error(format!("object of type '{}' has no len()", value.py_type_name(vm))))
}

/// A `randbelow` result, which always fits the `i64` length it was drawn below.
fn index_value(index: u128) -> i64 {
    i64::try_from(index).expect("index below an i64 length")
}

// ============================================================================
// Real-valued distributions
// ============================================================================

/// `uniform(a, b)` — a Python `def`.
#[derive(FromArgs)]
#[from_args(name = "Random.uniform", style = def)]
struct UniformArgs {
    a: Value,
    b: Value,
}

/// `uniform(a, b)`: `a + (b - a) * random()`.
fn uniform(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let UniformArgs { a, b } = UniformArgs::from_args(args, vm)?;
    defer_drop!(a, vm);
    defer_drop!(b, vm);
    let (a, b) = (to_float(a, vm)?, to_float(b, vm)?);
    let r = target.with_generator(vm, |random, _| random.rng().random());
    Ok(Value::Float(a + (b - a) * r))
}

/// `triangular(low=0.0, high=1.0, mode=None)` — a Python `def`.
#[derive(FromArgs)]
#[from_args(name = "Random.triangular", style = def)]
struct TriangularArgs {
    #[from_args(default = Value::Float(0.0))]
    low: Value,
    #[from_args(default = Value::Float(1.0))]
    high: Value,
    #[from_args(default = Value::None)]
    mode: Value,
}

/// `triangular(low=0.0, high=1.0, mode=None)`: the triangular distribution;
/// `low == high` short-circuits to `low` where CPython catches its
/// `ZeroDivisionError`.
fn triangular(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let TriangularArgs { low, high, mode } = TriangularArgs::from_args(args, vm)?;
    defer_drop!(low, vm);
    defer_drop!(high, vm);
    defer_drop!(mode, vm);
    let (mut low, mut high) = (to_float(low, vm)?, to_float(high, vm)?);
    let mode = if matches!(mode, Value::None) {
        None
    } else {
        Some(to_float(mode, vm)?)
    };

    let mut u = target.with_generator(vm, |random, _| random.rng().random());
    let mut c = match mode {
        None => 0.5,
        Some(_) if high - low == 0.0 => return Ok(Value::Float(low)),
        Some(mode) => (mode - low) / (high - low),
    };
    if u > c {
        u = 1.0 - u;
        c = 1.0 - c;
        mem::swap(&mut low, &mut high);
    }
    Ok(Value::Float(low + (high - low) * (u * c).sqrt()))
}

/// `normalvariate(mu=0.0, sigma=1.0)` / `gauss(mu=0.0, sigma=1.0)` — Python `def`s.
#[derive(FromArgs)]
#[from_args(name = "Random.normalvariate", style = def)]
struct NormalArgs {
    #[from_args(default = Value::Float(0.0))]
    mu: Value,
    #[from_args(default = Value::Float(1.0))]
    sigma: Value,
}

/// `normalvariate(mu=0.0, sigma=1.0)`: Kinderman–Monahan ratio of uniforms.
fn normalvariate(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let NormalArgs { mu, sigma } = NormalArgs::from_args(args, vm)?;
    defer_drop!(mu, vm);
    defer_drop!(sigma, vm);
    let (mu, sigma) = (to_float(mu, vm)?, to_float(sigma, vm)?);
    Ok(Value::Float(target.with_generator(vm, |random, _| {
        mu + normal_deviate(random.rng()) * sigma
    })))
}

/// One standard normal deviate by `normalvariate`'s rejection loop.
fn normal_deviate(rng: &mut Mt19937) -> f64 {
    // `NV_MAGICCONST = 4 * exp(-0.5) / sqrt(2.0)`, evaluated the same way.
    let nv_magicconst = 4.0 * (-0.5f64).exp() / 2f64.sqrt();
    loop {
        let u1 = rng.random();
        let u2 = 1.0 - rng.random();
        let z = nv_magicconst * (u1 - 0.5) / u2;
        let zz = z * z / 4.0;
        if zz <= -u2.ln() {
            return z;
        }
    }
}

/// `gauss(mu=0.0, sigma=1.0)`: Box–Muller, keeping the second deviate for
/// the next call.
fn gauss(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let NormalArgs { mu, sigma } = NormalArgs::from_args(args, vm)?;
    defer_drop!(mu, vm);
    defer_drop!(sigma, vm);
    let (mu, sigma) = (to_float(mu, vm)?, to_float(sigma, vm)?);
    Ok(Value::Float(target.with_generator(vm, |random, _| {
        let z = if let Some(z) = random.take_gauss_next() {
            z
        } else {
            let rng = random.rng();
            let x2pi = rng.random() * TAU;
            let g2rad = (-2.0 * (1.0 - rng.random()).ln()).sqrt();
            random.set_gauss_next(Some(x2pi.sin() * g2rad));
            x2pi.cos() * g2rad
        };
        mu + z * sigma
    })))
}

/// `lognormvariate(mu, sigma)` — a Python `def`.
#[derive(FromArgs)]
#[from_args(name = "Random.lognormvariate", style = def)]
struct LognormArgs {
    mu: Value,
    sigma: Value,
}

/// `lognormvariate(mu, sigma)`: `exp(normalvariate(mu, sigma))`.
fn lognormvariate(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let LognormArgs { mu, sigma } = LognormArgs::from_args(args, vm)?;
    defer_drop!(mu, vm);
    defer_drop!(sigma, vm);
    let (mu, sigma) = (to_float(mu, vm)?, to_float(sigma, vm)?);
    Ok(Value::Float(target.with_generator(vm, |random, _| {
        (mu + normal_deviate(random.rng()) * sigma).exp()
    })))
}

/// `expovariate(lambd=1.0)` — a Python `def`.
#[derive(FromArgs)]
#[from_args(name = "Random.expovariate", style = def)]
struct ExpovariateArgs {
    #[from_args(default = Value::Float(1.0))]
    lambd: Value,
}

/// `expovariate(lambd=1.0)`: `-log(1 - random()) / lambd`.
fn expovariate(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let ExpovariateArgs { lambd } = ExpovariateArgs::from_args(args, vm)?;
    defer_drop!(lambd, vm);
    let lambd = to_float(lambd, vm)?;
    let r = target.with_generator(vm, |random, _| random.rng().random());
    let numerator = -(1.0 - r).ln();
    Ok(Value::Float(float_div(numerator, lambd)?))
}

/// `vonmisesvariate(mu, kappa)` — a Python `def`.
#[derive(FromArgs)]
#[from_args(name = "Random.vonmisesvariate", style = def)]
struct VonmisesArgs {
    mu: Value,
    kappa: Value,
}

/// `vonmisesvariate(mu, kappa)`: Fisher's circular distribution algorithm.
#[expect(
    clippy::many_single_char_names,
    reason = "the paper's names, as random.py keeps them"
)]
fn vonmisesvariate(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let VonmisesArgs { mu, kappa } = VonmisesArgs::from_args(args, vm)?;
    defer_drop!(mu, vm);
    defer_drop!(kappa, vm);
    let (mu, kappa) = (to_float(mu, vm)?, to_float(kappa, vm)?);
    Ok(Value::Float(target.with_generator(vm, |random, _| {
        let rng = random.rng();
        if kappa <= 1e-6 {
            return TAU * rng.random();
        }
        let s = 0.5 / kappa;
        let r = s + (1.0 + s * s).sqrt();
        let z = loop {
            let u1 = rng.random();
            let z = (PI * u1).cos();
            let d = z / (r + z);
            let u2 = rng.random();
            if u2 < 1.0 - d * d || u2 <= (1.0 - d) * d.exp() {
                break z;
            }
        };
        let q = 1.0 / r;
        let f = (q + z) / (1.0 + q * z);
        let u3 = rng.random();
        if u3 > 0.5 {
            py_fmod(mu + f.acos(), TAU)
        } else {
            py_fmod(mu - f.acos(), TAU)
        }
    })))
}

/// `gammavariate(alpha, beta)` / `betavariate(alpha, beta)` / `weibullvariate(alpha, beta)` — Python `def`s.
#[derive(FromArgs)]
#[from_args(name = "Random.gammavariate", style = def)]
struct AlphaBetaArgs {
    alpha: Value,
    beta: Value,
}

/// `gammavariate(alpha, beta)`: the gamma distribution, by Cheng's method
/// above `alpha == 1`, exponential at it, and Kennedy & Gentle's below.
fn gammavariate(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let AlphaBetaArgs { alpha, beta } = AlphaBetaArgs::from_args(args, vm)?;
    defer_drop!(alpha, vm);
    defer_drop!(beta, vm);
    let (alpha, beta) = (to_float(alpha, vm)?, to_float(beta, vm)?);
    if alpha <= 0.0 || beta <= 0.0 {
        return Err(ExcType::value_error("gammavariate: alpha and beta must be > 0.0"));
    }
    Ok(Value::Float(target.with_generator(vm, |random, _| {
        gamma_deviate(random.rng(), alpha, beta)
    })))
}

/// The `gammavariate` body for validated `alpha`/`beta`.
#[expect(
    clippy::float_cmp,
    reason = "`alpha == 1.0` is the exact branch condition in random.py"
)]
fn gamma_deviate(rng: &mut Mt19937, alpha: f64, beta: f64) -> f64 {
    let log4 = 4f64.ln();
    let sg_magicconst = 1.0 + 4.5f64.ln();
    if alpha > 1.0 {
        let ainv = (2.0 * alpha - 1.0).sqrt();
        let bbb = alpha - log4;
        let ccc = alpha + ainv;
        loop {
            let u1 = rng.random();
            if !(1e-7 < u1 && u1 < 0.999_999_9) {
                continue;
            }
            let u2 = 1.0 - rng.random();
            let v = (u1 / (1.0 - u1)).ln() / ainv;
            let x = alpha * v.exp();
            let z = u1 * u1 * u2;
            let r = bbb + ccc * v - x;
            if r + sg_magicconst - 4.5 * z >= 0.0 || r >= z.ln() {
                return x * beta;
            }
        }
    } else if alpha == 1.0 {
        -(1.0 - rng.random()).ln() * beta
    } else {
        loop {
            let u = rng.random();
            let b = (E + alpha) / E;
            let p = b * u;
            let x = if p <= 1.0 {
                p.powf(1.0 / alpha)
            } else {
                -((b - p) / alpha).ln()
            };
            let u1 = rng.random();
            let accept = if p > 1.0 {
                u1 <= x.powf(alpha - 1.0)
            } else {
                u1 <= (-x).exp()
            };
            if accept {
                return x * beta;
            }
        }
    }
}

/// `betavariate(alpha, beta)`: a ratio of two gamma deviates.
fn betavariate(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let AlphaBetaArgs { alpha, beta } = AlphaBetaArgs::from_args(args, vm)?;
    defer_drop!(alpha, vm);
    defer_drop!(beta, vm);
    let (alpha, beta) = (to_float(alpha, vm)?, to_float(beta, vm)?);
    // Both inner `gammavariate` calls validate; the first rejects `alpha`,
    // and `beta` only gets checked once `y` is non-zero, as in CPython.
    if alpha <= 0.0 {
        return Err(ExcType::value_error("gammavariate: alpha and beta must be > 0.0"));
    }
    let y = target.with_generator(vm, |random, _| gamma_deviate(random.rng(), alpha, 1.0));
    if y == 0.0 {
        return Ok(Value::Float(0.0));
    }
    if beta <= 0.0 {
        return Err(ExcType::value_error("gammavariate: alpha and beta must be > 0.0"));
    }
    let z = target.with_generator(vm, |random, _| gamma_deviate(random.rng(), beta, 1.0));
    Ok(Value::Float(y / (y + z)))
}

/// `paretovariate(alpha)` — a Python `def`.
#[derive(FromArgs)]
#[from_args(name = "Random.paretovariate", style = def)]
struct ParetoArgs {
    alpha: Value,
}

/// `paretovariate(alpha)`: `(1 - random()) ** (-1 / alpha)`.
fn paretovariate(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let ParetoArgs { alpha } = ParetoArgs::from_args(args, vm)?;
    defer_drop!(alpha, vm);
    let alpha = to_float(alpha, vm)?;
    let u = 1.0 - target.with_generator(vm, |random, _| random.rng().random());
    Ok(Value::Float(u.powf(float_div(-1.0, alpha)?)))
}

/// `weibullvariate(alpha, beta)`: `alpha * (-log(1 - random())) ** (1 / beta)`.
fn weibullvariate(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let AlphaBetaArgs { alpha, beta } = AlphaBetaArgs::from_args(args, vm)?;
    defer_drop!(alpha, vm);
    defer_drop!(beta, vm);
    let (alpha, beta) = (to_float(alpha, vm)?, to_float(beta, vm)?);
    let u = 1.0 - target.with_generator(vm, |random, _| random.rng().random());
    Ok(Value::Float(alpha * (-u.ln()).powf(float_div(1.0, beta)?)))
}

/// `binomialvariate(n=1, p=0.5)` — a Python `def`.
#[derive(FromArgs)]
#[from_args(name = "Random.binomialvariate", style = def)]
struct BinomialArgs {
    #[from_args(default = Value::Int(1))]
    n: Value,
    #[from_args(default = Value::Float(0.5))]
    p: Value,
}

/// `binomialvariate(n=1, p=0.5)`: successes in `n` trials — Devroye's
/// geometric method for small `n * p`, Hörmann's BTRS otherwise.
#[expect(
    clippy::float_cmp,
    reason = "`p == 0.0` / `p == 1.0` are exact edge cases in random.py"
)]
fn binomialvariate(target: RandomTarget, args: ArgValues, vm: &mut VM<'_>) -> RunResult<Value> {
    let BinomialArgs { n, p } = BinomialArgs::from_args(args, vm)?;
    defer_drop!(n, vm);
    defer_drop!(p, vm);
    let n = n.as_int(vm)?;
    let p = to_float(p, vm)?;
    if n < 0 {
        return Err(ExcType::value_error("n must be non-negative"));
    }
    if p <= 0.0 || p >= 1.0 {
        return if p == 0.0 {
            Ok(Value::Int(0))
        } else if p == 1.0 {
            Ok(Value::Int(n))
        } else {
            Err(ExcType::value_error("p must be in the range 0.0 <= p <= 1.0"))
        };
    }
    Ok(Value::Int(
        target.with_generator(vm, |random, _| binomial_deviate(random.rng(), n, p)),
    ))
}

/// The `binomialvariate` body for validated `n` and `0 < p < 1`.
#[expect(clippy::cast_possible_truncation, reason = "floors of values bounded by n")]
#[expect(clippy::cast_precision_loss, reason = "n as f64, as CPython computes n * p")]
#[expect(
    clippy::many_single_char_names,
    reason = "the paper's names, as random.py keeps them"
)]
fn binomial_deviate(rng: &mut Mt19937, n: i64, p: f64) -> i64 {
    if n == 1 {
        return i64::from(rng.random() < p);
    }
    // Exploit symmetry to establish p <= 0.5.
    if p > 0.5 {
        return n - binomial_deviate(rng, n, 1.0 - p);
    }
    let n_f = n as f64;
    if n_f * p < 10.0 {
        // BG: Devroye's geometric method, O(np).
        let mut x: i64 = 0;
        let mut y: i64 = 0;
        let c = (1.0 - p).log2();
        if c == 0.0 {
            return x;
        }
        loop {
            let r = rng.random();
            if r == 0.0 {
                // `log2(0.0)` raises in CPython, which retries.
                continue;
            }
            y = y.saturating_add((r.log2() / c).floor() as i64).saturating_add(1);
            if y > n {
                return x;
            }
            x += 1;
        }
    }

    // BTRS: transformed rejection with squeeze (Hörmann).
    let spq = (n_f * p * (1.0 - p)).sqrt();
    let b = 1.15 + 2.53 * spq;
    let a = -0.0873 + 0.0248 * b + 0.01 * p;
    let c = n_f * p + 0.5;
    let vr = 0.92 - 4.2 / b;
    let mut setup: Option<(f64, f64, f64, f64)> = None;
    loop {
        let u = rng.random() - 0.5;
        let us = 0.5 - u.abs();
        if us == 0.0 {
            // A zero `us` divides by zero in CPython, which retries.
            continue;
        }
        let k = ((2.0 * a / us + b) * u + c).floor();
        if k < 0.0 || k > n_f {
            continue;
        }
        let k_int = k as i64;
        let mut v = rng.random();
        if us >= 0.07 && v <= vr {
            return k_int;
        }
        let (alpha, lpq, m, h) = *setup.get_or_insert_with(|| {
            let alpha = (2.83 + 5.1 / b) * spq;
            let lpq = (p / (1.0 - p)).ln();
            let m = ((n_f + 1.0) * p).floor();
            let h = libm::lgamma(m + 1.0) + libm::lgamma(n_f - m + 1.0);
            (alpha, lpq, m, h)
        });
        v *= alpha / (a / (us * us) + b);
        if v.ln() <= h - libm::lgamma(k + 1.0) - libm::lgamma(n_f - k + 1.0) + (k - m) * lpq {
            return k_int;
        }
    }
}

/// Python's `float % float`: the result takes the divisor's sign.
fn py_fmod(a: f64, b: f64) -> f64 {
    let r = a % b;
    if r != 0.0 && (r < 0.0) != (b < 0.0) { r + b } else { r }
}

/// Python's `float / float`, raising on a zero divisor.
fn float_div(numerator: f64, divisor: f64) -> RunResult<f64> {
    if divisor == 0.0 {
        Err(ExcType::zero_division().into())
    } else {
        Ok(numerator / divisor)
    }
}

/// Coerces a distribution parameter to a float, as the arithmetic in
/// `random.py` would; non-numbers raise `math`'s wording.
fn to_float(value: &Value, vm: &VM<'_>) -> RunResult<f64> {
    match value {
        Value::Float(f) => Ok(*f),
        #[expect(clippy::cast_precision_loss, reason = "int to float, as Python arithmetic does")]
        Value::Int(i) => Ok(*i as f64),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        _ => match value.as_long_int(vm) {
            Some(big) => bigint_to_f64_checked(big),
            None => Err(ExcType::type_error(format!(
                "must be real number, not {}",
                value.py_type_name(vm)
            ))),
        },
    }
}
