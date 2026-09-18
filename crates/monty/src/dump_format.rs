//! Versioned framing for serialized interpreter state.
//!
//! A dump is one postcard value — [`Dump`] — carrying both the interpreter
//! state and the session metadata a host must restore alongside it (script
//! name, type-check stubs), behind a `[MAGIC][DUMP_VERSION]` header. There is
//! exactly one dump shape, so hosts need no format knowledge beyond [`dump`]
//! and [`Dump::load`]; whether the session was idle or suspended is the
//! [`Session`] discriminant, not a separate tag.

use std::{error::Error, fmt, mem::size_of};

use monty_types::TypeCheckState;
use postcard::ser_flavors::Flavor;
use serde::{Deserialize, Serialize};

use crate::{
    repl::{MontyRepl, ReplProgress},
    run_progress::RunProgress,
};

/// Prefix distinguishing Monty dumps from unframed postcard data.
const MAGIC: &[u8; 6] = b"MONTY\0";

/// Version of the dump's postcard schema.
///
/// Bump this for every release where a serialized discriminant can shift, so older dumps are
/// rejected instead of decoding as their neighbour. That covers the
/// interpreter's own types *and* everything reachable from [`Dump`] — notably
/// [`TypeCheckingConfig`](monty_types::TypeCheckingConfig) in `monty-types`.
///
/// Before bumping, check there's already been a bump since the last release - multiple bumps
/// between releases is unnecessary and can lead to confusion.
pub const DUMP_VERSION: u16 = 11;

/// Set to [`DUMP_VERSION`], the current dump version, until this crate can load older dumps.
pub const MIN_SUPPORTED_DUMP_VERSION: u16 = DUMP_VERSION;

// The supported range must be non-empty, and must exclude zero
const _: () = assert!(MIN_SUPPORTED_DUMP_VERSION >= 1);
const _: () = assert!(MIN_SUPPORTED_DUMP_VERSION <= DUMP_VERSION);

/// Number of bytes before the postcard payload.
const HEADER_LEN: usize = MAGIC.len() + size_of::<u16>();

/// Initial payload capacity for [`dump`]. A fresh idle session dumps to ~130
/// bytes and one suspended on a host call to ~480, so this never over-allocates
/// meaningfully and skips the first few `Vec` doublings.
const MIN_PAYLOAD_CAPACITY: usize = 200;

/// Serializes a live session and its metadata into a versioned dump, readable
/// by [`Dump::load`].
///
/// Takes the state by reference because dumping is read-only: the caller keeps
/// its session and can carry on feeding it.
///
/// # Errors
/// Returns an error if serialization fails.
pub fn dump(
    script_name: &str,
    type_check: Option<&TypeCheckState>,
    state: SessionRef<'_>,
) -> Result<Vec<u8>, postcard::Error> {
    /// Borrowed mirror of [`Dump`]; postcard encodes it identically.
    #[derive(Serialize)]
    struct DumpRef<'a> {
        script_name: &'a str,
        type_check: Option<&'a TypeCheckState>,
        state: SessionRef<'a>,
    }

    let mut bytes = Vec::with_capacity(HEADER_LEN + MIN_PAYLOAD_CAPACITY);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&DUMP_VERSION.to_le_bytes());
    // the payload is written after the header in place: no second buffer to copy it into
    let dump = DumpRef {
        script_name,
        type_check,
        state,
    };
    postcard::serialize_with_flavor(&dump, PrefixedVec(bytes))
}

/// Postcard output flavor appending to a `Vec` that already holds the dump
/// header. `postcard::to_extend` does the same through `Extend`, which
/// benchmarks ~10% slower than `Vec::push`/`extend_from_slice`.
struct PrefixedVec(Vec<u8>);

impl Flavor for PrefixedVec {
    type Output = Vec<u8>;

    #[inline]
    fn try_extend(&mut self, data: &[u8]) -> postcard::Result<()> {
        self.0.extend_from_slice(data);
        Ok(())
    }

    #[inline]
    fn try_push(&mut self, data: u8) -> postcard::Result<()> {
        self.0.push(data);
        Ok(())
    }

    fn finalize(self) -> postcard::Result<Self::Output> {
        Ok(self.0)
    }
}

/// A complete REPL session snapshot: the interpreter state plus the
/// session-scoped context that lives outside it.
///
/// The metadata travels with the state because a restored session is otherwise
/// silently downgraded — losing `script_name` corrupts tracebacks, and losing
/// `type_check` disables enforcement the parent asked for.
#[derive(Debug, Deserialize)]
pub struct Dump {
    /// Script name used for tracebacks and type-check diagnostics.
    pub script_name: String,
    /// `Some` when the session was created with type checking enabled.
    pub type_check: Option<TypeCheckState>,
    /// The interpreter state, and where it was paused.
    pub state: Session,
}

impl Dump {
    /// Restores a session dumped by [`dump`].
    ///
    /// # Snapshot trust
    /// The caller must establish that the bytes are unmodified output from a trusted,
    /// compatible Monty producer. Invalid snapshots have no correctness or availability
    /// guarantees: loading or using them may panic, abort, hang, or produce wrong results,
    /// but must not cause undefined behaviour in the host process.
    /// Successful decoding does not authenticate or fully validate a snapshot.
    /// The same contract applies to direct serde deserialization.
    ///
    /// Accepts [`MIN_SUPPORTED_DUMP_VERSION`]`..=`[`DUMP_VERSION`], which is one
    /// version wide until a compatibility mechanism lowers the floor.
    ///
    /// # Errors
    /// Returns [`DumpError`] for a dump this build cannot read. The version
    /// variants name the bound the dump missed, so a host can tell a stale
    /// snapshot from one written by a build it should be reading with.
    pub fn load(bytes: &[u8]) -> Result<Self, DumpError> {
        let Some(header) = bytes.get(..HEADER_LEN) else {
            return Err(DumpError::NotADump);
        };
        let version = u16::from_le_bytes([header[MAGIC.len()], header[MAGIC.len() + 1]]);
        if &header[..MAGIC.len()] != MAGIC {
            Err(DumpError::NotADump)
        } else if version < MIN_SUPPORTED_DUMP_VERSION {
            Err(DumpError::VersionTooOld {
                found: version,
                min_supported: MIN_SUPPORTED_DUMP_VERSION,
            })
        } else if version > DUMP_VERSION {
            Err(DumpError::VersionTooNew {
                found: version,
                max_supported: DUMP_VERSION,
            })
        } else {
            let (value, remainder) = postcard::take_from_bytes(&bytes[HEADER_LEN..]).map_err(DumpError::Payload)?;
            if remainder.is_empty() {
                Ok(value)
            } else {
                Err(DumpError::Payload(postcard::Error::DeserializeBadEncoding))
            }
        }
    }
}

/// Where a dumped session was paused. The variant order is mirrored by
/// [`SessionRef`] and encoded as a postcard discriminant — keep them in step.
///
/// Both arms are boxed because they differ by hundreds of bytes inline; a
/// `Box<T>` serializes exactly as `T`, so this does not change the wire form.
#[derive(Debug, Deserialize)]
pub enum Session {
    /// Between feeds, ready for the next snippet.
    Idle(Box<MontyRepl>),
    /// Mid-feed, waiting on a resume.
    Suspended(Box<ReplProgress>),
    /// A one-shot [`crate::MontyRun`] execution paused at a suspension. Not a
    /// repl, so it cannot be fed further — only resumed to completion.
    Running(Box<RunProgress>),
}

/// Borrowed counterpart of [`Session`] used when dumping, so a live session can
/// be serialized without moving the repl out of the host's own state.
#[derive(Debug, Serialize)]
pub enum SessionRef<'a> {
    /// Between feeds, ready for the next snippet.
    Idle(&'a MontyRepl),
    /// Mid-feed, waiting on a resume.
    Suspended(&'a ReplProgress),
    /// A paused one-shot [`crate::MontyRun`] execution.
    Running(&'a RunProgress),
}

/// Why a dump could not be restored.
///
/// The two version failures are separate variants because they need opposite
/// responses: a too-old dump is dead and its session must be rebuilt by
/// replaying feeds, while a too-new one is intact and wants a newer reader.
#[derive(Debug, PartialEq, Eq)]
pub enum DumpError {
    /// Too short to hold a header, or missing the magic prefix.
    NotADump,
    /// Written by a build older than the oldest this one reads.
    VersionTooOld {
        /// Version the dump was written with.
        found: u16,
        /// Oldest version this build reads.
        min_supported: u16,
    },
    /// Written by a newer build, so the bytes are worth keeping — a build at or
    /// above `found` reads them.
    VersionTooNew {
        /// Version the dump was written with.
        found: u16,
        /// Newest version this build reads.
        max_supported: u16,
    },
    /// A version this build reads, holding something it cannot load — reserved
    /// for a compatibility mechanism and not produced today. `reason` names what
    /// blocked it; the remedy matches [`Self::VersionTooOld`].
    Unsupported {
        /// Version the dump was written with.
        found: u16,
        /// What this build could not load, for a host to log.
        reason: String,
    },
    /// Header was valid but the postcard payload did not decode.
    Payload(postcard::Error),
}

impl fmt::Display for DumpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotADump => write!(f, "not a monty dump"),
            Self::VersionTooOld { found, min_supported } => {
                write!(
                    f,
                    "dump format version {found} is older than {min_supported}, the oldest this build reads"
                )
            }
            Self::VersionTooNew { found, max_supported } => {
                write!(
                    f,
                    "dump format version {found} is newer than {max_supported}, the newest this build reads"
                )
            }
            Self::Unsupported { found, reason } => {
                write!(f, "dump format version {found} is unsupported: {reason}")
            }
            Self::Payload(err) => write!(f, "malformed dump payload: {err}"),
        }
    }
}

impl Error for DumpError {}

#[cfg(test)]
mod tests {
    use monty_types::{BuiltinsFunctions, MontyType, TypeCheckingFormat};
    use strum::VariantNames;

    use super::DUMP_VERSION;
    use crate::{bytecode::opcode_fingerprint, expressions::comparison_operators_fingerprint, types::Type};

    /// If a component changes incompatibly, bump `DUMP_VERSION` before updating its
    /// expected fingerprint. Compatible changes only require a fingerprint update.
    ///
    /// NB this test is not exhaustive of all possible compatibility issues, it just helps
    /// catch the obvious ones!
    #[test]
    fn serialized_components_match_dump_version() {
        assert_eq!(
            opcode_fingerprint(),
            0xc121_c3a3_9312_15ad,
            "opcodes changed for dump version {DUMP_VERSION}, actual: {}",
            grouped_hex(opcode_fingerprint())
        );
        assert_eq!(
            comparison_operators_fingerprint(),
            0x8ecc_d26b_160d_9c0b,
            "comparison operators changed for dump version {DUMP_VERSION}, actual: {}",
            grouped_hex(comparison_operators_fingerprint())
        );
        // `VariantNames` keeps the `#[strum(disabled)]` variants that `EnumString`
        // and `EnumIter` drop, which is what lets the two fingerprints below cover
        // every postcard discriminant. Asserted rather than assumed, so a strum
        // upgrade that changed it says so instead of quietly narrowing the guard.
        assert!(Type::VARIANTS.contains(&"instance"));
        assert!(MontyType::VARIANTS.contains(&"exception"));

        assert_eq!(
            variant_order_fingerprint(Type::VARIANTS),
            0xdb83_e6a5_fcb3_9768,
            "Type variants changed for dump version {DUMP_VERSION}, actual: {}",
            grouped_hex(variant_order_fingerprint(Type::VARIANTS))
        );
        assert_eq!(
            variant_order_fingerprint(MontyType::VARIANTS),
            0x0e43_247e_0759_a195,
            "MontyType variants changed for dump version {DUMP_VERSION}, actual: {}",
            grouped_hex(variant_order_fingerprint(MontyType::VARIANTS))
        );
        // Builtin discriminants are `CallBuiltinFunction` operands, so the enum
        // is append-only: a new builtin goes after the last variant.
        assert_eq!(
            variant_order_fingerprint(BuiltinsFunctions::VARIANTS),
            0xd5ef_68ff_fc6b_f752,
            "BuiltinsFunctions variants changed for dump version {DUMP_VERSION}, actual: {}",
            grouped_hex(variant_order_fingerprint(BuiltinsFunctions::VARIANTS))
        );
    }

    /// Formats an integer as hex with underscores between four-digit groups.
    fn grouped_hex(n: u64) -> String {
        let mut s = format!("{n:x}");
        for i in (1..s.len()).rev().skip(3).step_by(4) {
            s.insert(i, '_');
        }
        format!("0x{s}")
    }

    /// FNV-1a over variant names in declaration order.
    ///
    /// `Type` and `MontyType` are postcard-encoded by variant index inside a
    /// `Dump`, and `BuiltinsFunctions` discriminants are bytecode operands, so
    /// inserting a variant rewrites what older dumps decode to rather than
    /// failing the version check. Appending leaves this unchanged for every
    /// existing variant; inserting or reordering does not.
    ///
    /// The list covers the `#[strum(disabled)]` variants too — `Type::Instance`
    /// and `MontyType::Exception` — which carry discriminants like any other
    /// despite having no name to round-trip through `EnumString`.
    fn variant_order_fingerprint(variants: &[&str]) -> u64 {
        const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0100_0000_01b3;

        let mut hash = OFFSET_BASIS;
        for name in variants {
            for byte in name.as_bytes() {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(PRIME);
            }
            hash ^= 0xff;
            hash = hash.wrapping_mul(PRIME);
        }
        hash
    }

    /// `TypeCheckingFormat` reaches the dump schema through
    /// `monty_types::TypeCheckState` and serializes by discriminant, so inserting
    /// a variant rewrites older dumps' format rather than failing to decode.
    /// Append new variants at the end, or bump `DUMP_VERSION`.
    #[test]
    fn type_checking_format_variants_match_dump_version() {
        assert_eq!(
            TypeCheckingFormat::VARIANTS,
            [
                "full",
                "concise",
                "azure",
                "json",
                "jsonlines",
                "rdjson",
                "pylint",
                "gitlab",
                "github"
            ],
            "TypeCheckingFormat variants changed for dump version {DUMP_VERSION}"
        );
    }
}
