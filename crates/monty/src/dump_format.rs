//! Versioned framing for serialized interpreter state.
//!
//! A dump is one CBOR value — [`Dump`] — carrying both the interpreter
//! state and the session metadata a host must restore alongside it (script
//! name, type-check stubs), behind a `[MAGIC][DUMP_VERSION]` header. There is
//! exactly one dump shape, so hosts need no format knowledge beyond [`dump`]
//! and [`Dump::load`]; whether the session was idle or suspended is the
//! [`Session`] discriminant, not a separate tag.
//!
//! # Naming contract
//!
//! The payload is self-describing: structs are maps keyed by field name and
//! enums are keyed by variant name, so serde's derive evolves the schema on
//! its own. A dumped type may gain a field with `#[serde(default)]`, lose a
//! field, or have variants inserted anywhere, and older dumps still load.
//! Tuple structs and tuple variants are positional, so their fields may only
//! be appended.
//! The names themselves are the contract: renaming a serialized field or
//! variant needs `#[serde(alias = "old")]` (or a [`DUMP_VERSION`] bump), and
//! `#[serde(deny_unknown_fields)]` must never be added to a dumped type.

use std::{convert::Infallible, error::Error, fmt, mem::size_of};

use minicbor_serde::{
    Deserializer, Serializer,
    error::{DecodeError, EncodeError},
};
use monty_types::TypeCheckState;
use serde::{Deserialize, Serialize, de::Error as _};

use crate::{
    repl::{MontyRepl, ReplProgress},
    run_progress::RunProgress,
};

/// Prefix distinguishing Monty dumps from unframed CBOR data.
const MAGIC: &[u8; 6] = b"MONTY\0";

/// Version of the dump schema.
///
/// The payload names its fields and variants, so adding a field with a default,
/// removing one or reordering them does not need a bump (see the module docs).
/// Bump for every release where
/// the *meaning* of serialized data changes: opcodes or their operand shapes,
/// `BuiltinsFunctions` order (its discriminants are bytecode operands),
/// `CmpOperator` values, the compiler's constant layout, or a semantic change to
/// a stored value. Older dumps are then rejected instead of misexecuting.
/// Hashes are not stored: dicts and sets rebuild theirs on first use after a
/// load, so the hasher may change freely.
///
/// Before bumping, check there's already been a bump since the last release - multiple bumps
/// between releases is unnecessary and can lead to confusion.
pub const DUMP_VERSION: u16 = 13;

/// Set to [`DUMP_VERSION`], the current dump version, until this crate can load older dumps.
pub const MIN_SUPPORTED_DUMP_VERSION: u16 = DUMP_VERSION;

// The supported range must be non-empty, and must exclude zero
const _: () = assert!(MIN_SUPPORTED_DUMP_VERSION >= 1);
const _: () = assert!(MIN_SUPPORTED_DUMP_VERSION <= DUMP_VERSION);

/// Number of bytes before the CBOR payload.
const HEADER_LEN: usize = MAGIC.len() + size_of::<u16>();

/// Initial payload capacity for [`dump`]. A fresh idle session dumps to ~800
/// bytes and one suspended on a host call to ~2,400, so this never over-allocates
/// meaningfully and skips the first few `Vec` doublings.
const MIN_PAYLOAD_CAPACITY: usize = 1024;

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
) -> Result<Vec<u8>, DumpEncodeError> {
    /// Borrowed mirror of [`Dump`]; serde encodes it identically.
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
    dump.serialize(&mut Serializer::new(&mut bytes))
        .map_err(DumpEncodeError)?;
    Ok(bytes)
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
            let payload = &bytes[HEADER_LEN..];
            let mut deserializer = Deserializer::new(payload);
            let value = Self::deserialize(&mut deserializer).map_err(|err| DumpError::Payload(DumpDecodeError(err)))?;
            // trailing bytes are rejected rather than ignored, so a padded dump
            // cannot decode as the shorter valid one it starts with
            if deserializer.into_decoder().position() == payload.len() {
                Ok(value)
            } else {
                Err(DumpError::Payload(DumpDecodeError::custom(
                    "trailing bytes after the payload",
                )))
            }
        }
    }
}

/// Where a dumped session was paused. Variants are encoded by name and mirrored
/// by [`SessionRef`] — keep the two sets of names in step.
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
    /// Header was valid but the payload did not decode.
    Payload(DumpDecodeError),
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

/// Why a dump payload did not decode: a truncated or corrupt encoding, or a
/// value the interpreter refused to reconstruct.
///
/// Wraps the codec's error so the public API does not name the codec. Two
/// errors are equal when they render the same — the codec offers no structured
/// comparison, and hosts only ever see the message.
#[derive(Debug)]
pub struct DumpDecodeError(DecodeError);

impl DumpDecodeError {
    /// Wraps a message about the payload, for checks that run after decoding.
    fn custom(message: &'static str) -> Self {
        Self(DecodeError::custom(message))
    }
}

impl PartialEq for DumpDecodeError {
    fn eq(&self, other: &Self) -> bool {
        self.to_string() == other.to_string()
    }
}

impl Eq for DumpDecodeError {}

impl fmt::Display for DumpDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Error for DumpDecodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

/// Why [`dump`] could not serialize a session. Writing into memory cannot run
/// out of space, so this only surfaces a `Serialize` impl refusing a value.
#[derive(Debug)]
pub struct DumpEncodeError(EncodeError<Infallible>);

impl fmt::Display for DumpEncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Error for DumpEncodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use monty_types::{BuiltinsFunctions, ExcType, MontyType, TypeCheckingFormat};
    use serde::Serialize;
    use strum::VariantNames;

    use super::DUMP_VERSION;
    use crate::{
        bytecode::opcode_fingerprint, expressions::comparison_operators_fingerprint, heap::HeapId, types::Type,
    };

    /// If a component changes incompatibly, bump `DUMP_VERSION` before updating its
    /// expected fingerprint. Compatible changes only require a fingerprint update.
    ///
    /// NB this test is not exhaustive of all possible compatibility issues, it just helps
    /// catch the obvious ones!
    #[test]
    fn serialized_components_match_dump_version() {
        assert_eq!(
            opcode_fingerprint(),
            0xa05b_38e4_12c3_61f8,
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
        // drops, so the counts below cover every serialized variant once the
        // disabled ones are supplied by hand. Asserted rather than assumed, so a
        // strum upgrade that changed it says so instead of quietly narrowing the guard.
        assert!(Type::VARIANTS.contains(&"instance"));
        assert!(MontyType::VARIANTS.contains(&"exception"));

        let type_names = serde_variant_names(
            Type::VARIANTS,
            &[
                Type::Instance(HeapId::from_index(0)),
                Type::Exception(ExcType::ValueError),
            ],
        );
        assert_eq!(
            variant_name_fingerprint(&type_names),
            0x9901_dcf3_9e42_3098,
            "Type variants changed for dump version {DUMP_VERSION}, actual: {}",
            grouped_hex(variant_name_fingerprint(&type_names))
        );
        let monty_type_names = serde_variant_names(MontyType::VARIANTS, &[MontyType::Exception(ExcType::ValueError)]);
        assert_eq!(
            variant_name_fingerprint(&monty_type_names),
            0x8af8_54dc_0bfe_1e6d,
            "MontyType variants changed for dump version {DUMP_VERSION}, actual: {}",
            grouped_hex(variant_name_fingerprint(&monty_type_names))
        );
        // Builtin discriminants are `CallBuiltinFunction` operands, so the enum
        // is append-only: a new builtin goes after the last variant.
        assert_eq!(
            variant_order_fingerprint(BuiltinsFunctions::VARIANTS),
            0xcdd8_09b1_2adc_3852,
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
    /// `BuiltinsFunctions` discriminants are bytecode operands, so inserting a
    /// variant rewrites what older dumps execute rather than failing the version
    /// check. Appending leaves this unchanged for every existing variant;
    /// inserting or reordering does not.
    fn variant_order_fingerprint(variants: &[&str]) -> u64 {
        fnv1a(variants)
    }

    /// FNV-1a over variant names in sorted order.
    ///
    /// `Type` and `MontyType` are encoded by variant name inside a `Dump`, so
    /// order is free but a rename or removal breaks older dumps: fix it with
    /// `#[serde(alias)]` or bump `DUMP_VERSION`. Adding a variant only needs
    /// the expected hash updated.
    fn variant_name_fingerprint(names: &[String]) -> u64 {
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        fnv1a(&sorted)
    }

    /// FNV-1a over a sequence of names, each terminated by a separator.
    fn fnv1a(names: &[impl AsRef<str>]) -> u64 {
        const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0100_0000_01b3;

        let mut hash = OFFSET_BASIS;
        for name in names {
            for byte in name.as_ref().as_bytes() {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(PRIME);
            }
            hash ^= 0xff;
            hash = hash.wrapping_mul(PRIME);
        }
        hash
    }

    /// The names serde writes for every variant of an enum, read back from the
    /// encoded form rather than taken from strum, whose `#[strum(serialize)]`
    /// spelling can stay put while the Rust name serde uses changes. `disabled`
    /// supplies the `#[strum(disabled)]` variants `EnumString` refuses to parse.
    fn serde_variant_names<T: FromStr + Serialize>(strum_names: &[&str], disabled: &[T]) -> Vec<String> {
        let parsed: Vec<T> = strum_names.iter().filter_map(|name| T::from_str(name).ok()).collect();
        assert_eq!(
            parsed.len() + disabled.len(),
            strum_names.len(),
            "every variant must be covered"
        );
        parsed
            .iter()
            .chain(disabled)
            .map(|variant| encoded_variant_name(&minicbor_serde::to_vec(variant).unwrap()))
            .collect()
    }

    /// The variant name at the front of an encoded enum value: a bare text string
    /// for a unit variant, or the single key of the map wrapping a payload.
    fn encoded_variant_name(bytes: &[u8]) -> String {
        let text = if bytes[0] == 0xa1 { &bytes[1..] } else { bytes };
        let (len, start) = match text[0] {
            header @ 0x60..=0x77 => (usize::from(header - 0x60), 1),
            0x78 => (usize::from(text[1]), 2),
            header => panic!("expected a text string, got CBOR header {header:#x}"),
        };
        String::from_utf8(text[start..start + len].to_vec()).unwrap()
    }

    /// `TypeCheckingFormat` reaches the dump schema through
    /// `monty_types::TypeCheckState` and serializes by variant name, so the
    /// names are compared in sorted order: renaming one needs `#[serde(alias)]`
    /// or a `DUMP_VERSION` bump, adding one only needs this list updated.
    #[test]
    fn type_checking_format_variants_match_dump_version() {
        let mut variants = serde_variant_names::<TypeCheckingFormat>(TypeCheckingFormat::VARIANTS, &[]);
        variants.sort_unstable();
        assert_eq!(
            variants,
            [
                "Azure",
                "Concise",
                "Full",
                "Github",
                "Gitlab",
                "Json",
                "JsonLines",
                "Pylint",
                "Rdjson"
            ],
            "TypeCheckingFormat variants changed for dump version {DUMP_VERSION}"
        );
    }
}
