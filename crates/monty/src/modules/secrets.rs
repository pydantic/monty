//! Cryptographically strong random values and token helpers.
//!
//! Entropy comes from the embedding host through `os.urandom`, never from
//! Monty's seeded `random` generator.

use monty_types::{OsFunctionCall, UrandomArgs};
use num_bigint::{BigUint, Sign};
use num_traits::Zero;
use subtle::ConstantTimeEq;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropGuard, HeapData, HeapId},
    intern::StaticStrings,
    modules::{ModuleFunctions, random},
    os_dispatch::{PreConversionEffect, value_as_bytes},
    types::{
        List, Module,
        str::{PyStr, allocate_string},
    },
    value::Value,
};

/// Dispatch targets for the built-in module's functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum SecretsFunctions {
    Randbits,
    Randbelow,
    Choice,
    TokenBytes,
    TokenHex,
    TokenUrlsafe,
    CompareDigest,
}

const DEFAULT_ENTROPY: i64 = 32;

/// The initial mutable `DEFAULT_ENTROPY` value shared by a VM session.
pub(crate) fn default_entropy_value() -> Value {
    Value::Int(DEFAULT_ENTROPY)
}

const FUNCTIONS: &[(StaticStrings, SecretsFunctions)] = &[
    (StaticStrings::Randbits, SecretsFunctions::Randbits),
    (StaticStrings::Randbelow, SecretsFunctions::Randbelow),
    (StaticStrings::Choice, SecretsFunctions::Choice),
    (StaticStrings::TokenBytes, SecretsFunctions::TokenBytes),
    (StaticStrings::TokenHex, SecretsFunctions::TokenHex),
    (StaticStrings::TokenUrlsafe, SecretsFunctions::TokenUrlsafe),
    (StaticStrings::CompareDigest, SecretsFunctions::CompareDigest),
];

/// Builds the built-in module with its functions and export list.
pub(crate) fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Secrets, vm.interns);
    for (name, function) in FUNCTIONS {
        module.set_attr(*name, Value::ModuleFunction(ModuleFunctions::Secrets(*function)), vm);
    }
    module.set_attr(StaticStrings::DefaultEntropy, default_entropy_value(), vm);
    let exported = [
        "choice",
        "randbelow",
        "randbits",
        "token_bytes",
        "token_hex",
        "token_urlsafe",
        "compare_digest",
    ]
    .into_iter()
    .map(|name| allocate_string(name, vm.heap))
    .collect();
    module.set_attr(
        StaticStrings::DunderAll,
        Value::Ref(vm.heap.allocate(HeapData::List(List::new(exported)))),
        vm,
    );
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

pub(super) fn call(vm: &mut VM<'_>, function: SecretsFunctions, args: ArgValues) -> RunResult<CallResult> {
    match function {
        SecretsFunctions::TokenBytes => token_bytes(vm, args, TokenOutput::Bytes),
        SecretsFunctions::TokenHex => token_bytes(vm, args, TokenOutput::Hex),
        SecretsFunctions::TokenUrlsafe => token_bytes(vm, args, TokenOutput::UrlSafe),
        SecretsFunctions::Randbits => randbits(vm, args),
        SecretsFunctions::Randbelow => randbelow(vm, args),
        SecretsFunctions::Choice => choice(vm, args),
        SecretsFunctions::CompareDigest => compare_digest(vm, args),
    }
}

/// Converts a host entropy reply into the requested `secrets` result.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum Output {
    Bytes,
    Hex,
    UrlSafe,
    Bits { bit_count: usize },
    Randbelow { upper: BigUint, bit_count: usize },
}

/// Output formats accepted only by the three token helper functions.
#[derive(Clone, Copy)]
enum TokenOutput {
    Bytes,
    Hex,
    UrlSafe,
}

#[derive(FromArgs)]
#[from_args(name = "token_bytes", style = def)]
struct TokenBytesArgs {
    #[from_args(default = Value::None)]
    nbytes: Value,
}

#[derive(FromArgs)]
#[from_args(name = "token_hex", style = def)]
struct TokenHexArgs {
    #[from_args(default = Value::None)]
    nbytes: Value,
}

#[derive(FromArgs)]
#[from_args(name = "token_urlsafe", style = def)]
struct TokenUrlSafeArgs {
    #[from_args(default = Value::None)]
    nbytes: Value,
}

fn token_bytes(vm: &mut VM<'_>, args: ArgValues, output: TokenOutput) -> RunResult<CallResult> {
    let nbytes = match output {
        TokenOutput::Bytes => TokenBytesArgs::from_args(args, vm)?.nbytes,
        TokenOutput::Hex => TokenHexArgs::from_args(args, vm)?.nbytes,
        TokenOutput::UrlSafe => TokenUrlSafeArgs::from_args(args, vm)?.nbytes,
    };
    let nbytes = if matches!(nbytes, Value::None) {
        vm.secrets_default_entropy.clone_with_heap(vm.heap)
    } else {
        nbytes
    };
    defer_drop!(nbytes, vm);
    let size = nbytes.as_int_with_overflow(vm, ExcType::overflow_c_ssize_t)?;
    if size < 0 {
        return Err(ExcType::value_error("negative argument not allowed"));
    }
    let size = usize::try_from(size).map_err(|_| ExcType::overflow_c_ssize_t())?;
    vm.heap.tracker.check_allocation(size)?;
    let output_size = match output {
        TokenOutput::Bytes => size,
        TokenOutput::Hex => size.saturating_mul(2),
        TokenOutput::UrlSafe => size.saturating_mul(4).div_ceil(3),
    };
    vm.heap.tracker.check_allocation(output_size)?;
    let output = match output {
        TokenOutput::Bytes => Output::Bytes,
        TokenOutput::Hex => Output::Hex,
        TokenOutput::UrlSafe => Output::UrlSafe,
    };
    request_entropy(vm, size, output)
}

#[derive(FromArgs)]
#[from_args(name = "randbits", style = def)]
struct BitsArgs {
    k: Value,
}

fn randbits(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let BitsArgs { k } = BitsArgs::from_args(args, vm)?;
    defer_drop!(k, vm);
    if let Value::Float(value) = &k {
        if *value < 0.0 {
            return Err(ExcType::value_error("number of bits must be non-negative"));
        }
        return Err(ExcType::type_error(
            "'float' object cannot be interpreted as an integer",
        ));
    }
    if k.as_long_int(vm).is_some_and(|value| value.sign() == Sign::Minus) {
        return Err(ExcType::value_error("number of bits must be non-negative"));
    }
    let bit_count = match k.as_int_with_overflow(vm, ExcType::overflow_c_ssize_t) {
        Ok(value) => value,
        Err(error) if error.is_catchable(ExcType::TypeError) => {
            return Err(ExcType::type_error(format!(
                "'<' not supported between instances of '{}' and 'int'",
                k.py_type_name(vm)
            )));
        }
        Err(error) => return Err(error),
    };
    if bit_count < 0 {
        return Err(ExcType::value_error("number of bits must be non-negative"));
    }
    let bit_count = usize::try_from(bit_count).map_err(|_| ExcType::overflow_c_ssize_t())?;
    let size = bit_count.saturating_add(7) / 8;
    request_entropy(vm, size, Output::Bits { bit_count })
}

#[derive(FromArgs)]
#[from_args(name = "randbelow", style = def)]
struct BelowArgs {
    #[from_args(static_string = "ExclusiveUpperBound")]
    exclusive_upper_bound: Value,
}

fn randbelow(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let BelowArgs { exclusive_upper_bound } = BelowArgs::from_args(args, vm)?;
    defer_drop!(exclusive_upper_bound, vm);
    let (upper, bit_count) = match &exclusive_upper_bound {
        Value::Int(value) if *value > 0 => {
            let upper = BigUint::from(u64::try_from(*value).expect("positive i64 fits in u64"));
            let bit_count = usize::try_from(upper.bits()).map_err(|_| ExcType::overflow_c_ssize_t())?;
            (upper, bit_count)
        }
        Value::Bool(value) => {
            let upper = BigUint::from(*value);
            let bit_count = usize::try_from(upper.bits()).map_err(|_| ExcType::overflow_c_ssize_t())?;
            if upper.is_zero() {
                return Err(ExcType::value_error("Upper bound must be positive."));
            }
            (upper, bit_count)
        }
        Value::Int(_) => return Err(ExcType::value_error("Upper bound must be positive.")),
        Value::Float(value) => {
            if *value <= 0.0 {
                return Err(ExcType::value_error("Upper bound must be positive."));
            }
            return Err(ExcType::attribute_error("float", "bit_length"));
        }
        _ => {
            let upper = exclusive_upper_bound.as_long_int(vm).ok_or_else(|| {
                ExcType::type_error(format!(
                    "'<=' not supported between instances of '{}' and 'int'",
                    exclusive_upper_bound.py_type_name(vm)
                ))
            })?;
            if upper.sign() != Sign::Plus {
                return Err(ExcType::value_error("Upper bound must be positive."));
            }
            let bit_count = usize::try_from(upper.bits()).map_err(|_| ExcType::overflow_c_ssize_t())?;
            let size = bit_count.saturating_add(7) / 8;
            vm.heap.tracker.check_allocation(size)?;
            (
                upper.to_biguint().expect("positive BigInt converts to BigUint"),
                bit_count,
            )
        }
    };
    let size = bit_count.saturating_add(7) / 8;
    request_entropy(vm, size, Output::Randbelow { upper, bit_count })
}

#[derive(FromArgs)]
#[from_args(name = "Random.choice", style = def)]
struct ChoiceArgs {
    seq: Value,
}

fn choice(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let ChoiceArgs { seq: sequence } = ChoiceArgs::from_args(args, vm)?;
    let mut sequence_guard = DropGuard::new(sequence, vm);
    let (sequence, vm) = sequence_guard.as_parts_mut();
    let len = random::len_of(sequence, vm)?;
    if len == 0 {
        return Err(ExcType::index_error("Cannot choose from an empty sequence"));
    }
    let bit_count = usize::BITS as usize - len.leading_zeros() as usize;
    let size = bit_count.saturating_add(7) / 8;
    vm.heap.tracker.check_allocation(size)?;
    let (sequence, _) = sequence_guard.into_parts();
    Ok(CallResult::OsCallWithEffect {
        call: OsFunctionCall::Urandom(UrandomArgs { size: size as u64 }),
        effect: PreConversionEffect::SecretChoice {
            sequence,
            bound: len,
            size,
            bits: bit_count,
        }
        .into(),
    })
}

fn request_entropy(vm: &mut VM<'_>, size: usize, output: Output) -> RunResult<CallResult> {
    vm.heap.tracker.check_allocation(size)?;
    Ok(CallResult::OsCallWithEffect {
        call: OsFunctionCall::Urandom(UrandomArgs { size: size as u64 }),
        effect: PreConversionEffect::SecretBytes { size, output }.into(),
    })
}

#[derive(FromArgs)]
#[from_args(name = "compare_digest", style = unpack, kwarg_error_name = "_hashlib.compare_digest")]
struct CompareArgs {
    #[from_args(pos_only)]
    a: Value,
    #[from_args(pos_only)]
    b: Value,
}

fn compare_digest(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let CompareArgs { a, b } = CompareArgs::from_args(args, vm)?;
    defer_drop!(a, vm);
    defer_drop!(b, vm);
    let a_bytes = value_as_bytes(a, vm.heap, vm.interns);
    let b_bytes = value_as_bytes(b, vm.heap, vm.interns);
    let equal = match (a_bytes, b_bytes) {
        (Some(a_bytes), Some(b_bytes)) => bool::from(a_bytes.ct_eq(b_bytes)),
        (None, None) => {
            let (Some(a_str), Some(b_str)) = (PyStr::from_value(a, vm), PyStr::from_value(b, vm)) else {
                return Err(ExcType::type_error(format!(
                    "unsupported operand types(s) or combination of types: '{}' and '{}'",
                    a.py_type_name(vm),
                    b.py_type_name(vm)
                )));
            };
            let a = a_str;
            let b = b_str;
            let a = a.as_str(vm).as_bytes();
            let b = b.as_str(vm).as_bytes();
            if !a.is_ascii() || !b.is_ascii() {
                return Err(ExcType::type_error(
                    "comparing strings with non-ASCII characters is not supported",
                ));
            }
            bool::from(a.ct_eq(b))
        }
        (Some(_), None) => {
            return Err(ExcType::type_error(format!(
                "a bytes-like object is required, not '{}'",
                b.py_type_name(vm)
            )));
        }
        (None, Some(_)) => {
            return Err(ExcType::type_error(format!(
                "a bytes-like object is required, not '{}'",
                a.py_type_name(vm)
            )));
        }
    };
    Ok(CallResult::Value(Value::Bool(equal)))
}
