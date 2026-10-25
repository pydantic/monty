//! Implementation of Python's `hashlib` module.
//!
//! The constructors, `new()`, `pbkdf2_hmac()` and the two `algorithms_*` sets
//! are implemented; `file_digest()` and `scrypt()` are not — see
//! `limitations/hashlib.md`. The hash objects themselves live in
//! `types::hashlib`, along with every algorithm, so nothing here depends on
//! OpenSSL: `algorithms_available` is exactly `algorithms_guaranteed`.
//!
//! CPython's constructors are C functions in `_hashlib` (OpenSSL-backed,
//! parsed by Argument Clinic) and `_blake2`, so their signatures use the
//! named C parser style and the bodies reproduce the order in which CPython
//! rejects bad values.

use std::fmt::Display;

use monty_types::ResourceTracker;
use num_bigint::Sign;
use num_traits::ToPrimitive;

use crate::{
    args::{ArgValues, FromArgs, KwargsValues, LaxBool, StrArg, long_int},
    builtins::Builtins,
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    heap::{ContainsHeap, DropWithContext, HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    types::{
        Module, Set, Type, allocate_string,
        bytes::allocate_bytes,
        hashlib::{Blake2Params, Blake2b, Blake2s, DigestBytes, HashAlgorithm, HashObject, hash_input},
    },
    value::Value,
};

/// `hashlib` module functions, one variant per Python-visible function.
///
/// `blake2b` and `blake2s` are types rather than functions, as in CPython, so
/// they are not here. Serialized into dumps by variant name, so renaming one
/// needs `#[serde(alias)]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum HashlibFunctions {
    Md5,
    Sha1,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
    Sha3_224,
    Sha3_256,
    Sha3_384,
    Sha3_512,
    #[strum(serialize = "shake_128")]
    Shake128,
    #[strum(serialize = "shake_256")]
    Shake256,
    New,
    Pbkdf2Hmac,
}

/// Static mapping of attribute names to functions for module creation.
const HASHLIB_FUNCTIONS: &[(StaticStrings, HashlibFunctions)] = &[
    (StaticStrings::Md5, HashlibFunctions::Md5),
    (StaticStrings::Sha1, HashlibFunctions::Sha1),
    (StaticStrings::Sha224, HashlibFunctions::Sha224),
    (StaticStrings::Sha256, HashlibFunctions::Sha256),
    (StaticStrings::Sha384, HashlibFunctions::Sha384),
    (StaticStrings::Sha512, HashlibFunctions::Sha512),
    (StaticStrings::Sha3_224, HashlibFunctions::Sha3_224),
    (StaticStrings::Sha3_256, HashlibFunctions::Sha3_256),
    (StaticStrings::Sha3_384, HashlibFunctions::Sha3_384),
    (StaticStrings::Sha3_512, HashlibFunctions::Sha3_512),
    (StaticStrings::Shake128, HashlibFunctions::Shake128),
    (StaticStrings::Shake256, HashlibFunctions::Shake256),
    (StaticStrings::New, HashlibFunctions::New),
    (StaticStrings::Pbkdf2Hmac, HashlibFunctions::Pbkdf2Hmac),
];

/// Creates the `hashlib` module on the heap.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Hashlib, vm.interns);
    for (name, func) in HASHLIB_FUNCTIONS {
        module.set_attr(*name, Value::ModuleFunction(ModuleFunctions::Hashlib(*func)), vm);
    }
    module.set_attr(
        StaticStrings::Blake2b,
        Value::Builtin(Builtins::Type(Type::Blake2b)),
        vm,
    );
    module.set_attr(
        StaticStrings::Blake2s,
        Value::Builtin(Builtins::Type(Type::Blake2s)),
        vm,
    );
    // Two sets, as in CPython, where mutating one leaves the other alone.
    for name in [StaticStrings::AlgorithmsGuaranteed, StaticStrings::AlgorithmsAvailable] {
        module.set_attr(name, algorithm_names(vm), vm);
    }
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// A fresh `set` of every algorithm name.
fn algorithm_names(vm: &mut VM<'_>) -> Value {
    let mut names = Set::with_capacity(HashAlgorithm::ALL.len());
    for algorithm in HashAlgorithm::ALL {
        let name = allocate_string(algorithm.name(), vm.heap);
        // Adding a str to a set of strs hashes and compares without Python
        // code, so the only failure is a memory limit already past its hard ceiling.
        names
            .add(name, vm)
            .expect("adding an interned name to a fresh set cannot raise");
    }
    Value::Ref(vm.heap.allocate(HeapData::Set(names)))
}

/// Dispatches a call to a `hashlib` module function.
pub(super) fn call(vm: &mut VM<'_>, function: HashlibFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        HashlibFunctions::Md5 => construct(HashAlgorithm::Md5, Md5Args::from_args(args, vm)?.into(), vm),
        HashlibFunctions::Sha1 => construct(HashAlgorithm::Sha1, Sha1Args::from_args(args, vm)?.into(), vm),
        HashlibFunctions::Sha224 => construct(HashAlgorithm::Sha224, Sha224Args::from_args(args, vm)?.into(), vm),
        HashlibFunctions::Sha256 => construct(HashAlgorithm::Sha256, Sha256Args::from_args(args, vm)?.into(), vm),
        HashlibFunctions::Sha384 => construct(HashAlgorithm::Sha384, Sha384Args::from_args(args, vm)?.into(), vm),
        HashlibFunctions::Sha512 => construct(HashAlgorithm::Sha512, Sha512Args::from_args(args, vm)?.into(), vm),
        HashlibFunctions::Sha3_224 => construct(HashAlgorithm::Sha3_224, Sha3_224Args::from_args(args, vm)?.into(), vm),
        HashlibFunctions::Sha3_256 => construct(HashAlgorithm::Sha3_256, Sha3_256Args::from_args(args, vm)?.into(), vm),
        HashlibFunctions::Sha3_384 => construct(HashAlgorithm::Sha3_384, Sha3_384Args::from_args(args, vm)?.into(), vm),
        HashlibFunctions::Sha3_512 => construct(HashAlgorithm::Sha3_512, Sha3_512Args::from_args(args, vm)?.into(), vm),
        HashlibFunctions::Shake128 => construct(HashAlgorithm::Shake128, Shake128Args::from_args(args, vm)?.into(), vm),
        HashlibFunctions::Shake256 => construct(HashAlgorithm::Shake256, Shake256Args::from_args(args, vm)?.into(), vm),
        HashlibFunctions::New => call_new(vm, args),
        HashlibFunctions::Pbkdf2Hmac => call_pbkdf2_hmac(vm, args),
    }
}

// ============================================================================
// The OpenSSL-backed constructors and `new()`
// ============================================================================

/// `data=b'', *, usedforsecurity=True, string=None`, the signature every
/// OpenSSL-backed constructor shares once its name is in the error messages.
///
/// `usedforsecurity` is accepted and ignored: there is no FIPS mode to honour.
struct ConstructorArgs {
    data: Option<Value>,
    usedforsecurity: Value,
    string: Option<Value>,
}

impl<C: ContainsHeap> DropWithContext<C> for ConstructorArgs {
    fn drop_with(self, ctx: &mut C) {
        self.data.drop_with(ctx);
        self.usedforsecurity.drop_with(ctx);
        self.string.drop_with(ctx);
    }
}

/// Stamps out one `FromArgs` struct per constructor, since the derive bakes
/// the function name (`openssl_sha256() takes at most …`) into the spec.
macro_rules! openssl_constructor_args {
    ($($args:ident => $name:literal),* $(,)?) => {$(
        #[derive(FromArgs)]
        #[from_args(name = $name, style = c_named, at_most_total)]
        struct $args {
            #[from_args(default)]
            data: Option<Value>,
            #[from_args(kw_only, default = Value::Bool(true))]
            usedforsecurity: Value,
            #[from_args(kw_only, default, static_string = "StringAttr")]
            string: Option<Value>,
        }

        impl From<$args> for ConstructorArgs {
            fn from(args: $args) -> Self {
                let $args {
                    data,
                    usedforsecurity,
                    string,
                } = args;
                Self {
                    data,
                    usedforsecurity,
                    string,
                }
            }
        }
    )*};
}

openssl_constructor_args!(
    Md5Args => "openssl_md5",
    Sha1Args => "openssl_sha1",
    Sha224Args => "openssl_sha224",
    Sha256Args => "openssl_sha256",
    Sha384Args => "openssl_sha384",
    Sha512Args => "openssl_sha512",
    Sha3_224Args => "openssl_sha3_224",
    Sha3_256Args => "openssl_sha3_256",
    Sha3_384Args => "openssl_sha3_384",
    Sha3_512Args => "openssl_sha3_512",
    Shake128Args => "openssl_shake_128",
    Shake256Args => "openssl_shake_256",
);

/// Builds a hash of `algorithm` over the constructor's initial data.
fn construct(algorithm: HashAlgorithm, args: ConstructorArgs, vm: &mut VM<'_>) -> RunResult<Value> {
    defer_drop!(args, vm);
    let mut hash = HashObject::new(algorithm);
    if let Some(input) = initial_data(args.data.as_ref(), args.string.as_ref())? {
        hash.update(hash_input(input, vm)?, &vm.heap.tracker)?;
    }
    Ok(hash.allocate(vm.heap))
}

/// Picks the initial data between the `data` positional and the deprecated
/// `string` keyword, rejecting both at once as CPython does.
///
/// Passing either explicitly counts, even as `None`, which then fails the
/// buffer check.
fn initial_data<'a>(data: Option<&'a Value>, string: Option<&'a Value>) -> RunResult<Option<&'a Value>> {
    match (data, string) {
        (Some(_), Some(_)) => Err(ExcType::type_error(
            "'data' and 'string' are mutually exclusive and support for 'string' keyword parameter is slated for \
             removal in a future version.",
        )),
        (Some(data), None) => Ok(Some(data)),
        (None, string) => Ok(string),
    }
}

/// `hashlib.new(name, *args, **kwargs)`, a pure-Python `def` that routes
/// BLAKE2 names to the `_blake2` types and everything else to `_hashlib.new`.
#[derive(FromArgs)]
#[from_args(name = "__hash_new", style = def)]
struct HashNewArgs {
    name: Value,
    #[from_args(varargs)]
    args: Vec<Value>,
    #[from_args(varkwargs)]
    kwargs: KwargsValues,
}

/// `_hashlib.new(name, data=b'', *, usedforsecurity=True, string=None)`.
#[derive(FromArgs)]
#[from_args(name = "new", style = c_named, bad_arg_named, at_most_total)]
struct OpensslNewArgs {
    name: StrArg,
    #[from_args(default)]
    data: Option<Value>,
    #[from_args(kw_only, default = Value::Bool(true))]
    usedforsecurity: Value,
    #[from_args(kw_only, default, static_string = "StringAttr")]
    string: Option<Value>,
}

/// `hashlib.new()`: the name is checked for a BLAKE2 constructor first, so
/// `new('blake2b', digest_size=16)` reaches `blake2b()`'s own parser; an
/// OpenSSL alias such as `'blake2b512'` takes the plain path, as in CPython.
fn call_new(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let HashNewArgs { name, args, kwargs } = HashNewArgs::from_args(args, vm)?;
    let blake2 = name.to_str(vm).ok().and_then(Blake2Kind::from_name);
    if let Some(kind) = blake2 {
        name.drop_with(vm);
        return blake2_init(kind, vm, ArgValues::from_parts(args, kwargs));
    }
    // The name rejoins the binder's own vector; `insert` may still grow it,
    // but nothing is copied into a fresh buffer before the parser's checks.
    let mut positional = args;
    positional.insert(0, name);
    let OpensslNewArgs {
        name,
        data,
        usedforsecurity,
        string,
    } = OpensslNewArgs::from_args(ArgValues::from_parts(positional, kwargs), vm)?;
    defer_drop!(name, vm);
    defer_drop!(data, vm);
    defer_drop!(usedforsecurity, vm);
    defer_drop!(string, vm);
    // The data is checked before the name is resolved, as `_hashlib.new` does.
    let input = initial_data(data.as_ref(), string.as_ref())?
        .map(|input| hash_input(input, vm))
        .transpose()?;
    let name = name.as_str(vm);
    let algorithm = HashAlgorithm::from_name(name).ok_or_else(|| unsupported_hash_type(name))?;
    let mut hash = HashObject::new(algorithm);
    if let Some(input) = input {
        hash.update(input, &vm.heap.tracker)?;
    }
    Ok(hash.allocate(vm.heap))
}

/// `ValueError: unsupported hash type <name>`, from `hashlib.new()`.
fn unsupported_hash_type(name: &str) -> RunError {
    ExcType::value_error(format!("unsupported hash type {name}"))
}

// ============================================================================
// BLAKE2
// ============================================================================

/// Which of the two `_blake2` types a constructor call is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Blake2Kind {
    Blake2b,
    Blake2s,
}

impl Blake2Kind {
    /// The kind `hashlib.new(name)` hands to `_blake2`; exact and lowercase,
    /// unlike the OpenSSL names.
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "blake2b" => Some(Self::Blake2b),
            "blake2s" => Some(Self::Blake2s),
            _ => None,
        }
    }

    /// The capitalized name the `digest_size` message uses.
    fn display_name(self) -> &'static str {
        match self {
            Self::Blake2b => "Blake2b",
            Self::Blake2s => "Blake2s",
        }
    }

    fn max_digest_size(self) -> u8 {
        match self {
            Self::Blake2b => Blake2b::MAX_DIGEST_SIZE,
            Self::Blake2s => Blake2s::MAX_DIGEST_SIZE,
        }
    }

    fn salt_size(self) -> usize {
        match self {
            Self::Blake2b => Blake2b::SALT_SIZE,
            Self::Blake2s => Blake2s::SALT_SIZE,
        }
    }

    fn max_node_offset(self) -> u64 {
        match self {
            Self::Blake2b => Blake2b::MAX_NODE_OFFSET,
            Self::Blake2s => Blake2s::MAX_NODE_OFFSET,
        }
    }
}

/// The constructor arguments before conversion. Every parameter stays a raw
/// `Value` so the body can convert them in signature order, reproducing
/// which error CPython's parser reports first.
struct Blake2RawArgs {
    data: Option<Value>,
    digest_size: Option<Value>,
    key: Option<Value>,
    salt: Option<Value>,
    person: Option<Value>,
    fanout: Option<Value>,
    depth: Option<Value>,
    leaf_size: Option<Value>,
    node_offset: Option<Value>,
    node_depth: Option<Value>,
    inner_size: Option<Value>,
    last_node: bool,
    usedforsecurity: Value,
    string: Option<Value>,
}

impl<C: ContainsHeap> DropWithContext<C> for Blake2RawArgs {
    fn drop_with(self, ctx: &mut C) {
        self.usedforsecurity.drop_with(ctx);
        for value in [
            self.data,
            self.digest_size,
            self.key,
            self.salt,
            self.person,
            self.fanout,
            self.depth,
            self.leaf_size,
            self.node_offset,
            self.node_depth,
            self.inner_size,
            self.string,
        ] {
            value.drop_with(ctx);
        }
    }
}

/// Stamps out the two `_blake2` constructor signatures, which differ only in
/// the name their errors carry.
macro_rules! blake2_constructor_args {
    ($($args:ident => $name:literal),* $(,)?) => {$(
        #[derive(FromArgs)]
        #[from_args(name = $name, style = c_named, at_most_total)]
        struct $args {
            #[from_args(default)]
            data: Option<Value>,
            #[from_args(kw_only, default)]
            digest_size: Option<Value>,
            #[from_args(kw_only, default)]
            key: Option<Value>,
            #[from_args(kw_only, default)]
            salt: Option<Value>,
            #[from_args(kw_only, default)]
            person: Option<Value>,
            #[from_args(kw_only, default)]
            fanout: Option<Value>,
            #[from_args(kw_only, default)]
            depth: Option<Value>,
            #[from_args(kw_only, default)]
            leaf_size: Option<Value>,
            #[from_args(kw_only, default)]
            node_offset: Option<Value>,
            #[from_args(kw_only, default)]
            node_depth: Option<Value>,
            #[from_args(kw_only, default)]
            inner_size: Option<Value>,
            #[from_args(kw_only, default = LaxBool::new(false))]
            last_node: LaxBool,
            #[from_args(kw_only, default = Value::Bool(true))]
            usedforsecurity: Value,
            #[from_args(kw_only, default, static_string = "StringAttr")]
            string: Option<Value>,
        }

        impl From<$args> for Blake2RawArgs {
            fn from(args: $args) -> Self {
                let $args {
                    data,
                    digest_size,
                    key,
                    salt,
                    person,
                    fanout,
                    depth,
                    leaf_size,
                    node_offset,
                    node_depth,
                    inner_size,
                    last_node,
                    usedforsecurity,
                    string,
                } = args;
                Self {
                    data,
                    digest_size,
                    key,
                    salt,
                    person,
                    fanout,
                    depth,
                    leaf_size,
                    node_offset,
                    node_depth,
                    inner_size,
                    last_node: last_node.bool(),
                    usedforsecurity,
                    string,
                }
            }
        }
    )*};
}

blake2_constructor_args!(Blake2bArgs => "blake2b", Blake2sArgs => "blake2s");

/// `blake2b(...)` / `blake2s(...)`, also reached through `hashlib.new()`.
///
/// Conversions run in signature order, then the parameter checks in the
/// order `_blake2` makes them, and the data last.
pub(crate) fn blake2_init(kind: Blake2Kind, vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let args: Blake2RawArgs = match kind {
        Blake2Kind::Blake2b => Blake2bArgs::from_args(args, vm)?.into(),
        Blake2Kind::Blake2s => Blake2sArgs::from_args(args, vm)?.into(),
    };
    defer_drop!(args, vm);
    let max_digest = kind.max_digest_size();
    let digest_size = args
        .digest_size
        .as_ref()
        .map_or(Ok(i32::from(max_digest)), |v| c_int(v, vm))?;
    let key = args.key.as_ref().map_or(Ok(&[][..]), |v| buffer(v, vm))?;
    let salt = args.salt.as_ref().map_or(Ok(&[][..]), |v| buffer(v, vm))?;
    let person = args.person.as_ref().map_or(Ok(&[][..]), |v| buffer(v, vm))?;
    let fanout = args.fanout.as_ref().map_or(Ok(1), |v| c_int(v, vm))?;
    let depth = args.depth.as_ref().map_or(Ok(1), |v| c_int(v, vm))?;
    let leaf_size = args
        .leaf_size
        .as_ref()
        .map_or(Ok(0), |v| c_unsigned(v, "unsigned long", vm))?;
    let node_offset = args
        .node_offset
        .as_ref()
        .map_or(Ok(0), |v| c_unsigned(v, "unsigned long long", vm))?;
    let node_depth = args.node_depth.as_ref().map_or(Ok(0), |v| c_int(v, vm))?;
    let inner_size = args.inner_size.as_ref().map_or(Ok(0), |v| c_int(v, vm))?;
    let input = initial_data(args.data.as_ref(), args.string.as_ref())?;

    let digest_size = u8::try_from(digest_size)
        .ok()
        .filter(|size| (1..=max_digest).contains(size))
        .ok_or_else(|| {
            ExcType::value_error(format!(
                "digest_size for {} must be between 1 and {max_digest} bytes, here it is {digest_size}",
                kind.display_name()
            ))
        })?;
    let salt_size = kind.salt_size();
    if salt.len() > salt_size {
        return Err(ExcType::value_error(format!(
            "maximum salt length is {salt_size} bytes"
        )));
    }
    if person.len() > salt_size {
        return Err(ExcType::value_error(format!(
            "maximum person length is {salt_size} bytes"
        )));
    }
    let fanout = u8::try_from(fanout).map_err(|_| ExcType::value_error("fanout must be between 0 and 255"))?;
    let depth = u8::try_from(depth)
        .ok()
        .filter(|depth| *depth >= 1)
        .ok_or_else(|| ExcType::value_error("depth must be between 1 and 255"))?;
    let leaf_size = u32::try_from(leaf_size).map_err(|_| overflow_error("leaf_size is too large"))?;
    if node_offset > kind.max_node_offset() {
        return Err(overflow_error("node_offset is too large"));
    }
    let node_depth =
        u8::try_from(node_depth).map_err(|_| ExcType::value_error("node_depth must be between 0 and 255"))?;
    let inner_size = u8::try_from(inner_size)
        .ok()
        .filter(|size| *size <= max_digest)
        .ok_or_else(|| ExcType::value_error(format!("inner_size must be between 0 and is {max_digest}")))?;
    if key.len() > usize::from(max_digest) {
        return Err(ExcType::value_error(format!(
            "maximum key length is {max_digest} bytes"
        )));
    }
    let input = input.map(|input| hash_input(input, vm)).transpose()?;

    let params = Blake2Params {
        digest_size,
        key,
        salt,
        person,
        fanout,
        depth,
        leaf_size,
        node_offset,
        node_depth,
        inner_size,
        last_node: args.last_node,
    };
    let mut hash = match kind {
        Blake2Kind::Blake2b => HashObject::blake2b(params),
        Blake2Kind::Blake2s => HashObject::blake2s(params),
    };
    if let Some(input) = input {
        hash.update(input, &vm.heap.tracker)?;
    }
    Ok(hash.allocate(vm.heap))
}

/// Borrows a `bytes` argument with the `Py_buffer` converter's message, which
/// differs from the data argument's (see [`hash_input`]).
fn buffer<'a>(value: &'a Value, vm: &'a VM<'_>) -> RunResult<&'a [u8]> {
    match value {
        Value::InternBytes(id) => Ok(vm.interns.get_bytes(*id)),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Bytes(bytes) => Ok(bytes.as_slice()),
            _ => Err(not_bytes_like(value, vm)),
        },
        _ => Err(not_bytes_like(value, vm)),
    }
}

/// `TypeError: a bytes-like object is required, not 'X'`.
fn not_bytes_like(value: &Value, vm: &VM<'_>) -> RunError {
    ExcType::type_error(format!(
        "a bytes-like object is required, not '{}'",
        value.py_type_name(vm)
    ))
}

/// Argument Clinic's `int` converter: a C `int`, overflowing with
/// `PyLong_AsInt`'s message.
fn c_int(value: &Value, vm: &VM<'_>) -> RunResult<i32> {
    match value {
        Value::Bool(b) => Ok(i32::from(*b)),
        Value::Int(i) => i32::try_from(*i).map_err(|_| overflow_error("Python int too large to convert to C int")),
        _ if long_int(value, vm).is_some() => Err(overflow_error("Python int too large to convert to C int")),
        _ => Err(ExcType::type_error_not_integer(&value.py_type_name(vm))),
    }
}

/// Argument Clinic's `long` converter: a C `long`, overflowing with
/// `PyLong_AsLong`'s message.
fn c_long(value: &Value, vm: &VM<'_>) -> RunResult<i64> {
    match value {
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::Int(i) => Ok(*i),
        _ if long_int(value, vm).is_some() => Err(ExcType::overflow_c_long()),
        _ => Err(ExcType::type_error_not_integer(&value.py_type_name(vm))),
    }
}

/// Argument Clinic's `unsigned_long` / `unsigned_long_long` converters,
/// which reject a negative int before checking the width named by `c_type`.
fn c_unsigned(value: &Value, c_type: &str, vm: &VM<'_>) -> RunResult<u64> {
    match value {
        Value::Bool(b) => Ok(u64::from(*b)),
        Value::Int(i) => u64::try_from(*i).map_err(|_| ExcType::value_error_negative_int()),
        _ => match long_int(value, vm) {
            Some(big) if big.sign() == Sign::Minus => Err(ExcType::value_error_negative_int()),
            Some(big) => big
                .to_u64()
                .ok_or_else(|| overflow_error(format!("Python int too large for C {c_type}"))),
            None => Err(ExcType::type_error_not_integer(&value.py_type_name(vm))),
        },
    }
}

/// An `OverflowError` with a custom message.
fn overflow_error(msg: impl Display) -> RunError {
    SimpleException::new_msg(ExcType::OverflowError, msg).into()
}

// ============================================================================
// pbkdf2_hmac
// ============================================================================

/// `pbkdf2_hmac(hash_name, password, salt, iterations, dklen=None)`, a clinic
/// function whose `password`, `salt` and `iterations` convert in the body so
/// their errors come out in signature order.
#[derive(FromArgs)]
#[from_args(name = "pbkdf2_hmac", style = c_named, bad_arg_named, at_most_total)]
struct Pbkdf2HmacArgs {
    hash_name: StrArg,
    password: Value,
    salt: Value,
    iterations: Value,
    #[from_args(default = Value::None)]
    dklen: Value,
}

/// `hashlib.pbkdf2_hmac()`: PBKDF2 (RFC 8018) over HMAC of the named
/// algorithm, checked in CPython's order — the algorithm, then `iterations`,
/// then `dklen`.
fn call_pbkdf2_hmac(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let Pbkdf2HmacArgs {
        hash_name,
        password,
        salt,
        iterations,
        dklen,
    } = Pbkdf2HmacArgs::from_args(args, vm)?;
    defer_drop!(hash_name, vm);
    defer_drop!(password, vm);
    defer_drop!(salt, vm);
    defer_drop!(iterations, vm);
    defer_drop!(dklen, vm);
    let password = buffer(password, vm)?;
    let salt = buffer(salt, vm)?;
    let iterations = c_long(iterations, vm)?;
    let name = hash_name.as_str(vm);
    let algorithm = HashAlgorithm::from_name(name).ok_or_else(|| unsupported_hash_type(name))?;
    if iterations < 1 {
        return Err(ExcType::value_error("iteration value must be greater than 0."));
    }
    // OpenSSL takes both as C ints.
    let iterations = i32::try_from(iterations).map_err(|_| overflow_error("iteration value is too great."))?;
    let dklen = if matches!(dklen, Value::None) {
        // A SHAKE's digest size is zero, which fails the same check an
        // explicit zero does.
        algorithm.digest_size()
    } else {
        let dklen = c_long(dklen, vm)?;
        let dklen = i32::try_from(dklen.max(0)).map_err(|_| overflow_error("key length is too great."))?;
        usize::try_from(dklen).expect("non-negative")
    };
    if dklen == 0 {
        return Err(ExcType::value_error("key length must be greater than 0."));
    }
    if algorithm.is_xof() {
        return Err(ExcType::value_error("[Provider routines] xof digests not allowed"));
    }
    vm.heap.tracker.check_allocation(dklen)?;
    let key = pbkdf2(algorithm, password, salt, iterations, dklen, &vm.heap.tracker)?;
    Ok(allocate_bytes(key, vm.heap))
}

/// PBKDF2 with HMAC-`algorithm` as the pseudorandom function.
///
/// `iterations` is caller-chosen, so each HMAC round polls the deadline; the
/// salt is absorbed once rather than re-hashed for every output block.
fn pbkdf2(
    algorithm: HashAlgorithm,
    password: &[u8],
    salt: &[u8],
    iterations: i32,
    dklen: usize,
    tracker: &ResourceTracker,
) -> RunResult<Vec<u8>> {
    let hmac = Hmac::new(algorithm, password, tracker)?;
    // Every block's first message is `salt || INT(block_index)`.
    let salted = hmac.with_prefix(salt, tracker)?;
    let mut key = Vec::with_capacity(dklen);
    let mut round = 0usize;
    for block_index in 1u32.. {
        let mut u = salted.sign(&block_index.to_be_bytes(), tracker)?;
        let mut t = u;
        for _ in 1..iterations {
            tracker.check_time_every(round)?;
            round = round.wrapping_add(1);
            u = hmac.sign(&u, tracker)?;
            for (acc, byte) in t.iter_mut().zip(u.iter()) {
                *acc ^= byte;
            }
        }
        key.extend(t.iter().take(dklen - key.len()));
        if key.len() == dklen {
            break;
        }
        tracker.check_time_every(round)?;
        round = round.wrapping_add(1);
    }
    Ok(key)
}

/// HMAC (RFC 2104) keyed once, so each message costs two cloned states and
/// no key preparation; with a stack digest, a SHA-2 round allocates nothing.
struct Hmac {
    /// The hash with the inner padded key absorbed.
    inner: HashObject,
    /// The hash with the outer padded key absorbed.
    outer: HashObject,
}

impl Hmac {
    fn new(algorithm: HashAlgorithm, key: &[u8], tracker: &ResourceTracker) -> RunResult<Self> {
        let block_size = algorithm.block_size();
        let mut padded = vec![0u8; block_size];
        if key.len() > block_size {
            let mut hashed = HashObject::new(algorithm);
            hashed.update(key, tracker)?;
            let digest = hashed.digest();
            padded[..digest.len()].copy_from_slice(&digest);
        } else {
            padded[..key.len()].copy_from_slice(key);
        }
        let keyed = |pad: u8| -> RunResult<HashObject> {
            let mut hash = HashObject::new(algorithm);
            let block: Vec<u8> = padded.iter().map(|byte| byte ^ pad).collect();
            hash.update(&block, tracker)?;
            Ok(hash)
        };
        Ok(Self {
            inner: keyed(0x36)?,
            outer: keyed(0x5c)?,
        })
    }

    /// The same MAC with `prefix` already absorbed, for signing many
    /// messages that share it.
    fn with_prefix(&self, prefix: &[u8], tracker: &ResourceTracker) -> RunResult<Self> {
        let mut inner = self.inner.clone();
        inner.update(prefix, tracker)?;
        Ok(Self {
            inner,
            outer: self.outer.clone(),
        })
    }

    /// The MAC of `message`.
    fn sign(&self, message: &[u8], tracker: &ResourceTracker) -> RunResult<DigestBytes> {
        let mut inner = self.inner.clone();
        inner.update(message, tracker)?;
        let mut outer = self.outer.clone();
        outer.update(&inner.digest(), tracker)?;
        Ok(outer.digest())
    }
}
