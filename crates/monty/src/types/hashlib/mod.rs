//! The hash objects `hashlib` hands out: `_hashlib.HASH`, `_hashlib.HASHXOF`,
//! `_blake2.blake2b` and `_blake2.blake2s`.
//!
//! A hash mid-stream is serializable state, so a dump taken between
//! `update()` calls restores and finishes to the same digest. SHA-1 and SHA-2
//! come from the RustCrypto crates for their hardware-accelerated
//! compression; MD5, SHA-3 and BLAKE2 are implemented here, where a crate
//! would be no faster (and BLAKE2's has no serializable state).
//! [`HashAlgorithm`] names the algorithm and carries the sizes CPython
//! reports; [`HashCore`] is the streaming state behind it.

mod blake2;
mod block;
mod keccak;
mod md5;
mod rustcrypto;

use std::{
    fmt::Write,
    mem,
    ops::{Deref, DerefMut},
};

use monty_types::ResourceTracker;
use serde::{Deserialize, Serialize};

pub(crate) use self::blake2::{Blake2Params, Blake2b, Blake2s};
use self::{
    keccak::{Keccak, SUFFIX_SHA3, SUFFIX_SHAKE},
    md5::Md5,
    rustcrypto::{Sha1, Sha224, Sha256, Sha384, Sha512},
};
use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    hash::{HashValue, identity_hash},
    heap::{DropWithContext, Heap, HeapData, HeapId, HeapItem, HeapObjectRead, HeapRead},
    intern::StaticStrings,
    types::{LazyHeapSet, PyTrait, Type, allocate_string, bytes::allocate_bytes, py_trait::PyObjectIdentity},
    value::{EitherStr, Value},
};

/// Lowercase hex digits for `hexdigest()`.
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// Input hashed between deadline polls in [`HashObject::update`]: roughly
/// 100 µs of work, so that is how far a limit-sized input can overshoot.
const POLL_CHUNK: usize = 64 * 1024;

/// The longest fixed digest here (SHA-512, SHA3-512 and BLAKE2b).
const MAX_DIGEST_SIZE: usize = 64;

/// A fixed-output digest on the stack, so PBKDF2's HMAC rounds allocate nothing.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DigestBytes {
    bytes: [u8; MAX_DIGEST_SIZE],
    len: u8,
}

impl DigestBytes {
    /// Copies a digest of at most [`MAX_DIGEST_SIZE`] bytes.
    pub(crate) fn from_slice(digest: &[u8]) -> Self {
        let mut bytes = [0; MAX_DIGEST_SIZE];
        bytes[..digest.len()].copy_from_slice(digest);
        Self {
            bytes,
            len: u8::try_from(digest.len()).expect("digests are at most 64 bytes"),
        }
    }
}

impl Deref for DigestBytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

impl DerefMut for DigestBytes {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.bytes[..usize::from(self.len)]
    }
}

/// The algorithms `hashlib.algorithms_guaranteed` lists, which is also
/// everything Monty's `hashlib.new()` accepts.
///
/// Serialized into dumps by variant name, so renaming one needs `#[serde(alias)]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum HashAlgorithm {
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
    Shake128,
    Shake256,
    Blake2b,
    Blake2s,
}

impl HashAlgorithm {
    /// Every algorithm, in `algorithms_guaranteed` order.
    pub(crate) const ALL: [Self; 14] = [
        Self::Md5,
        Self::Sha1,
        Self::Sha224,
        Self::Sha256,
        Self::Sha384,
        Self::Sha512,
        Self::Sha3_224,
        Self::Sha3_256,
        Self::Sha3_384,
        Self::Sha3_512,
        Self::Shake128,
        Self::Shake256,
        Self::Blake2b,
        Self::Blake2s,
    ];

    /// The `name` attribute, which is also the `hashlib` constructor's name.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Md5 => "md5",
            Self::Sha1 => "sha1",
            Self::Sha224 => "sha224",
            Self::Sha256 => "sha256",
            Self::Sha384 => "sha384",
            Self::Sha512 => "sha512",
            Self::Sha3_224 => "sha3_224",
            Self::Sha3_256 => "sha3_256",
            Self::Sha3_384 => "sha3_384",
            Self::Sha3_512 => "sha3_512",
            Self::Shake128 => "shake_128",
            Self::Shake256 => "shake_256",
            Self::Blake2b => "blake2b",
            Self::Blake2s => "blake2s",
        }
    }

    /// Resolves a `hashlib.new()` / `pbkdf2_hmac()` name: the exact `hashlib`
    /// name, or one of OpenSSL's aliases for the same algorithm, which it
    /// matches case-insensitively (`'SHA256'`, `'sha-256'`, `'blake2b512'`).
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|algorithm| {
            algorithm.name() == name
                || algorithm
                    .openssl_aliases()
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(name))
        })
    }

    /// The names OpenSSL's `EVP_get_digestbyname` accepts for the algorithm,
    /// which `hashlib.new()` falls back to after its own table.
    fn openssl_aliases(self) -> &'static [&'static str] {
        match self {
            Self::Md5 => &["md5", "ssl3-md5"],
            Self::Sha1 => &["sha1", "sha-1", "ssl3-sha1"],
            Self::Sha224 => &["sha224", "sha-224", "sha2-224"],
            Self::Sha256 => &["sha256", "sha-256", "sha2-256"],
            Self::Sha384 => &["sha384", "sha-384", "sha2-384"],
            Self::Sha512 => &["sha512", "sha-512", "sha2-512"],
            Self::Sha3_224 => &["sha3-224"],
            Self::Sha3_256 => &["sha3-256"],
            Self::Sha3_384 => &["sha3-384"],
            Self::Sha3_512 => &["sha3-512"],
            Self::Shake128 => &["shake128", "shake-128"],
            Self::Shake256 => &["shake256", "shake-256"],
            Self::Blake2b => &["blake2b512", "blake2b-512"],
            Self::Blake2s => &["blake2s256", "blake2s-256"],
        }
    }

    /// The `digest_size` attribute; `0` for the extendable-output SHAKEs and
    /// the default for BLAKE2, whose instances may carry a smaller one.
    pub(crate) fn digest_size(self) -> usize {
        match self {
            Self::Md5 => md5::DIGEST_SIZE,
            Self::Sha1 => 20,
            Self::Sha224 | Self::Sha3_224 => 28,
            Self::Sha256 | Self::Sha3_256 | Self::Blake2s => 32,
            Self::Sha384 | Self::Sha3_384 => 48,
            Self::Sha512 | Self::Sha3_512 | Self::Blake2b => 64,
            Self::Shake128 | Self::Shake256 => 0,
        }
    }

    /// The `block_size` attribute: the compression block, or a sponge's rate.
    pub(crate) fn block_size(self) -> usize {
        match self {
            Self::Md5 | Self::Sha1 | Self::Sha224 | Self::Sha256 => 64,
            Self::Sha384 | Self::Sha512 => 128,
            Self::Sha3_224 => 144,
            Self::Sha3_256 | Self::Shake256 => 136,
            Self::Sha3_384 => 104,
            Self::Sha3_512 => 72,
            Self::Shake128 => 168,
            Self::Blake2b => Blake2b::BLOCK_SIZE,
            Self::Blake2s => Blake2s::BLOCK_SIZE,
        }
    }

    /// Whether `digest()` takes a length, as the SHAKEs' does.
    pub(crate) fn is_xof(self) -> bool {
        matches!(self, Self::Shake128 | Self::Shake256)
    }

    /// The Python type of this algorithm's hash objects.
    pub(crate) fn py_type(self) -> Type {
        match self {
            Self::Blake2b => Type::Blake2b,
            Self::Blake2s => Type::Blake2s,
            Self::Shake128 | Self::Shake256 => Type::HashlibHashXof,
            _ => Type::HashlibHash,
        }
    }

    /// The `TypeName.method` prefix of the methods' arity errors: CPython's
    /// `HASH.update()`, with `HASHXOF` inheriting `HASH`'s methods.
    fn method_prefix(self) -> &'static str {
        match self {
            Self::Blake2b => "blake2b",
            Self::Blake2s => "blake2s",
            _ => "HASH",
        }
    }
}

/// Streaming state of one hash; the Keccak variant serves every SHA-3 and
/// SHAKE, which differ only by sponge rate and output length.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum HashCore {
    Md5(Md5),
    Sha1(Sha1),
    Sha224(Sha224),
    Sha256(Sha256),
    Sha384(Sha384),
    Sha512(Sha512),
    Keccak(Keccak),
    Blake2b(Blake2b),
    Blake2s(Blake2s),
}

/// A `hashlib` hash object: the algorithm plus its state.
///
/// `digest()` never consumes the state, matching CPython, so every digest
/// finalizes a copy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HashObject {
    algorithm: HashAlgorithm,
    core: HashCore,
}

impl HashObject {
    /// A fresh hash with the algorithm's default parameters.
    pub(crate) fn new(algorithm: HashAlgorithm) -> Self {
        let core = match algorithm {
            HashAlgorithm::Md5 => HashCore::Md5(Md5::default()),
            HashAlgorithm::Sha1 => HashCore::Sha1(Sha1::new()),
            HashAlgorithm::Sha224 => HashCore::Sha224(Sha224::new()),
            HashAlgorithm::Sha256 => HashCore::Sha256(Sha256::new()),
            HashAlgorithm::Sha384 => HashCore::Sha384(Sha384::new()),
            HashAlgorithm::Sha512 => HashCore::Sha512(Sha512::new()),
            HashAlgorithm::Sha3_224 | HashAlgorithm::Sha3_256 | HashAlgorithm::Sha3_384 | HashAlgorithm::Sha3_512 => {
                HashCore::Keccak(Keccak::new(algorithm.block_size(), SUFFIX_SHA3))
            }
            HashAlgorithm::Shake128 | HashAlgorithm::Shake256 => {
                HashCore::Keccak(Keccak::new(algorithm.block_size(), SUFFIX_SHAKE))
            }
            HashAlgorithm::Blake2b => HashCore::Blake2b(Blake2b::new(Blake2Params::default_for(64))),
            HashAlgorithm::Blake2s => HashCore::Blake2s(Blake2s::new(Blake2Params::default_for(32))),
        };
        Self { algorithm, core }
    }

    /// A BLAKE2b with validated `params`.
    pub(crate) fn blake2b(params: Blake2Params<'_>) -> Self {
        Self {
            algorithm: HashAlgorithm::Blake2b,
            core: HashCore::Blake2b(Blake2b::new(params)),
        }
    }

    /// A BLAKE2s with validated `params`.
    pub(crate) fn blake2s(params: Blake2Params<'_>) -> Self {
        Self {
            algorithm: HashAlgorithm::Blake2s,
            core: HashCore::Blake2s(Blake2s::new(params)),
        }
    }

    pub(crate) fn algorithm(&self) -> HashAlgorithm {
        self.algorithm
    }

    /// The `digest_size` attribute, which BLAKE2 parameters can shrink.
    pub(crate) fn digest_size(&self) -> usize {
        match &self.core {
            HashCore::Blake2b(blake) => blake.digest_size(),
            HashCore::Blake2s(blake) => blake.digest_size(),
            _ => self.algorithm.digest_size(),
        }
    }

    /// Absorbs `data`, polling the deadline once per [`POLL_CHUNK`] so a
    /// large input cannot run far past the time limit; an input under one
    /// chunk never polls.
    pub(crate) fn update(&mut self, data: &[u8], tracker: &ResourceTracker) -> RunResult<()> {
        for (i, chunk) in data.chunks(POLL_CHUNK).enumerate() {
            if i > 0 {
                tracker.check_time()?;
            }
            self.absorb(chunk);
        }
        Ok(())
    }

    fn absorb(&mut self, data: &[u8]) {
        match &mut self.core {
            HashCore::Md5(core) => core.update(data),
            HashCore::Sha1(core) => core.update(data),
            HashCore::Sha224(core) => core.update(data),
            HashCore::Sha256(core) => core.update(data),
            HashCore::Sha384(core) => core.update(data),
            HashCore::Sha512(core) => core.update(data),
            HashCore::Keccak(core) => core.update(data),
            HashCore::Blake2b(core) => core.update(data),
            HashCore::Blake2s(core) => core.update(data),
        }
    }

    /// The fixed-size digest; a SHAKE yields `digest_size` (zero) bytes, so
    /// callers give those a length through [`Self::digest_xof`].
    pub(crate) fn digest(&self) -> DigestBytes {
        match &self.core {
            HashCore::Md5(core) => DigestBytes::from_slice(&core.digest()),
            HashCore::Sha1(core) => core.digest(),
            HashCore::Sha224(core) => core.digest(),
            HashCore::Sha256(core) => core.digest(),
            HashCore::Sha384(core) => core.digest(),
            HashCore::Sha512(core) => core.digest(),
            HashCore::Keccak(core) => DigestBytes::from_slice(&core.digest(self.algorithm.digest_size())),
            HashCore::Blake2b(core) => DigestBytes::from_slice(&core.digest()),
            HashCore::Blake2s(core) => DigestBytes::from_slice(&core.digest()),
        }
    }

    /// `length` bytes of a SHAKE's output. `length` is caller-chosen, so the
    /// squeeze polls the deadline; the caller preflights the allocation.
    pub(crate) fn digest_xof(&self, length: usize, tracker: &ResourceTracker) -> RunResult<Vec<u8>> {
        match &self.core {
            HashCore::Keccak(core) => core.squeeze(length, tracker),
            _ => unreachable!("digest_xof on a fixed-output hash"),
        }
    }

    pub(crate) fn allocate(self, heap: &Heap) -> Value {
        Value::Ref(heap.allocate(HeapData::HashObject(Box::new(self))))
    }
}

/// The placeholder `update()` leaves in the heap while the real state is
/// lifted out; an empty MD5 allocates nothing, and nothing can observe it.
impl Default for HashObject {
    fn default() -> Self {
        Self::new(HashAlgorithm::Md5)
    }
}

impl Blake2Params<'static> {
    /// The parameters of a plain `blake2b()` / `blake2s()` call.
    pub(crate) fn default_for(digest_size: u8) -> Self {
        Self {
            digest_size,
            key: &[],
            salt: &[],
            person: &[],
            fanout: 1,
            depth: 1,
            leaf_size: 0,
            node_offset: 0,
            node_depth: 0,
            inner_size: 0,
            last_node: false,
        }
    }
}

/// A size attribute as a Python int.
fn size_value(size: usize) -> Value {
    Value::Int(i64::try_from(size).expect("hash sizes are small"))
}

/// Lowercase hex of `data`, as `hexdigest()` returns it.
pub(crate) fn hex_string(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 2);
    for byte in data {
        out.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// `digest(length)` of a `_hashlib.HASHXOF`.
#[derive(FromArgs)]
#[from_args(name = "digest", style = c_named, at_most_total)]
struct XofDigestArgs {
    length: u64,
}

/// `hexdigest(length)` of a `_hashlib.HASHXOF`.
#[derive(FromArgs)]
#[from_args(name = "hexdigest", style = c_named, at_most_total)]
struct XofHexdigestArgs {
    length: u64,
}

// ============================================================================
// Heap object glue
// ============================================================================

impl HeapItem for HashObject {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // A hash owns no heap references.
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, HashObject> {
    fn py_type(&self, vm: &VM<'h>) -> Type {
        self.get(vm.heap).algorithm.py_type()
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    fn py_eq_impl(&self, _: &Value, _: &mut VM<'h>) -> RunResult<Option<bool>> {
        Ok(None)
    }

    /// Hash objects hash by identity, as any class without `__eq__` does.
    fn py_hash(&self, _: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        Ok(Some(identity_hash(self.id())))
    }

    /// `<sha256 _hashlib.HASH object @ 0x…>` for the OpenSSL-backed types and
    /// the default `<_blake2.blake2b object at 0x…>` for BLAKE2.
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let algorithm = self.get(vm.heap).algorithm;
        let address = self.py_identity().encoded();
        match algorithm.py_type() {
            Type::Blake2b | Type::Blake2s => write!(f, "<{} object at 0x{address:x}>", algorithm.py_type())?,
            py_type => write!(f, "<{} {py_type} object @ 0x{address:x}>", algorithm.name())?,
        }
        Ok(())
    }

    /// `name`, `digest_size` and `block_size`, plus the size constants a
    /// BLAKE2 instance inherits from its class.
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let Some(name) = attr.static_string(vm.interns) else {
            return Ok(None);
        };
        let hash = self.get(vm.heap);
        let value = match name {
            StaticStrings::Name => allocate_string(hash.algorithm.name(), vm.heap),
            StaticStrings::DigestSize => size_value(hash.digest_size()),
            StaticStrings::BlockSize => size_value(hash.algorithm.block_size()),
            _ => match hash.algorithm.py_type().class_constant(attr, vm) {
                Some(constant) => constant,
                None => return Ok(None),
            },
        };
        Ok(Some(CallResult::Value(value)))
    }

    fn py_set_attr(&mut self, name: &EitherStr, value: Value, vm: &mut VM<'h>) -> RunResult<()> {
        let readonly = matches!(
            name.static_string(vm.interns),
            Some(StaticStrings::Name | StaticStrings::DigestSize | StaticStrings::BlockSize)
        );
        if readonly {
            value.drop_with(vm);
            let type_name = self.py_type(vm).name(vm.heap, vm.interns);
            Err(ExcType::attribute_error_not_writable(
                name.as_str(vm.interns),
                &type_name,
            ))
        } else {
            value.drop_with(vm);
            let type_name = self.py_type_name(vm);
            Err(ExcType::attribute_error_no_setattr(&type_name, name.as_str(vm.interns)))
        }
    }

    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let algorithm = self.get(vm.heap).algorithm;
        let method = |name: &str| format!("{}.{name}", algorithm.method_prefix());
        match attr.static_string(vm.interns) {
            Some(StaticStrings::Update) => {
                if matches!(args, ArgValues::Kwargs(_) | ArgValues::ArgsKargs { .. }) {
                    args.drop_with(vm);
                    return Err(ExcType::type_error_no_kwargs(&method("update")));
                }
                let data = args.get_one_arg(&method("update"), vm.heap)?;
                defer_drop!(data, vm);
                // The input borrows the heap, so the state is lifted out while
                // it is absorbed and put back on every path.
                let mut hash = mem::take(self.get_mut(vm.heap));
                let absorbed = hash_input(data, vm).and_then(|input| hash.update(input, &vm.heap.tracker));
                *self.get_mut(vm.heap) = hash;
                absorbed.map(|()| CallResult::Value(Value::None))
            }
            Some(StaticStrings::Digest) if algorithm.is_xof() => {
                let XofDigestArgs { length } = XofDigestArgs::from_args(args, vm)?;
                let length = xof_length(length, 1, vm)?;
                let digest = self.get(vm.heap).digest_xof(length, &vm.heap.tracker)?;
                Ok(CallResult::Value(allocate_bytes(digest, vm.heap)))
            }
            Some(StaticStrings::Hexdigest) if algorithm.is_xof() => {
                let XofHexdigestArgs { length } = XofHexdigestArgs::from_args(args, vm)?;
                // The raw digest stays live while its hex is built.
                let length = xof_length(length, 3, vm)?;
                let digest = self.get(vm.heap).digest_xof(length, &vm.heap.tracker)?;
                Ok(CallResult::Value(allocate_string(hex_string(&digest), vm.heap)))
            }
            Some(StaticStrings::Digest) => {
                args.check_zero_args(&method("digest"), vm.heap)?;
                let digest = self.get(vm.heap).digest();
                Ok(CallResult::Value(allocate_bytes(digest.to_vec(), vm.heap)))
            }
            Some(StaticStrings::Hexdigest) => {
                args.check_zero_args(&method("hexdigest"), vm.heap)?;
                let digest = self.get(vm.heap).digest();
                Ok(CallResult::Value(allocate_string(hex_string(&digest), vm.heap)))
            }
            Some(StaticStrings::Copy) => {
                args.check_zero_args(&method("copy"), vm.heap)?;
                Ok(CallResult::Value(self.allocate_like(vm)))
            }
            _ => {
                args.drop_with(vm);
                Err(ExcType::attribute_error(self.py_type_name(vm), attr.as_str(vm.interns)))
            }
        }
    }
}

impl<'h> HeapRead<'h, HashObject> {
    /// Allocates a hash at the same point in the same stream, as `copy()` does.
    pub(crate) fn allocate_like(&self, vm: &mut VM<'h>) -> Value {
        self.get(vm.heap).clone().allocate(vm.heap)
    }
}

/// The longest SHAKE output, CPython's `_sha3` ceiling of `2**29` bytes.
const MAX_XOF_LENGTH: usize = 1 << 29;

/// Validates a SHAKE `digest()` / `hexdigest()` length with `_sha3`'s wording
/// and preflights the `bytes_per_output` bytes each output byte costs, so an
/// oversized request is a graceful `MemoryError` rather than a hard-limit kill.
fn xof_length(length: u64, bytes_per_output: usize, vm: &VM<'_>) -> RunResult<usize> {
    let length = usize::try_from(length)
        .ok()
        .filter(|&length| length < MAX_XOF_LENGTH)
        .ok_or_else(|| ExcType::value_error("length is too large"))?;
    vm.heap.tracker.check_allocation(length * bytes_per_output)?;
    Ok(length)
}

/// Borrows the bytes a hash accepts, with the two messages `_hashlib` uses:
/// `str` is told to encode first, anything else is not a buffer.
pub(crate) fn hash_input<'a>(value: &'a Value, vm: &'a VM<'_>) -> RunResult<&'a [u8]> {
    match value {
        Value::InternBytes(id) => Ok(vm.interns.get_bytes(*id)),
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::Bytes(bytes) => Ok(bytes.as_slice()),
            HeapData::Str(_) => Err(ExcType::type_error("Strings must be encoded before hashing")),
            _ => Err(ExcType::type_error("object supporting the buffer API required")),
        },
        Value::InternString(_) => Err(ExcType::type_error("Strings must be encoded before hashing")),
        _ => Err(ExcType::type_error("object supporting the buffer API required")),
    }
}
