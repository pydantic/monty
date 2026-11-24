//! Versioned framing for serialized interpreter state.
//!
//! A dump is one CBOR value — [`Dump`] — carrying both the interpreter
//! state and the session metadata a host must restore alongside it (script
//! name, type-check stubs), behind a `[MAGIC][DUMP_VERSION]` header. There is
//! exactly one dump shape, so hosts need no format knowledge beyond [`dump`]
//! and [`Dump::load`]; whether the session was idle or suspended is the
//! [`Session`] discriminant, not a separate tag.
//!
//! Inline builtin types serialize as `Type` variant names; `Builtins` reads the
//! shapes written before exception classes and `type` joined the native type table.
//! Numeric identities retain their explicit legacy codes independently of the dump encoding.
//! See the tests in `types/native.rs` and the legacy serialization fixture.
//!
//! # Compatibility contract
//!
//! Published releases with the same dump version must read and resume each
//! other's dumps. This guarantee does not extend to unreleased Git revisions.
//!
//! By default, Serde encodes named structs as maps keyed by field name and
//! externally tagged enums by variant name. Untagged enums, transparent newtypes
//! and custom serializers can use other layouts.
//! Adding a named field is compatible if newer readers accept its absence (e.g.
//! with `#[serde(default)]`) and older readers ignore it when present.
//! Removing a field requires older readers to tolerate its absence.
//! Writing a new variant can break older readers. An alias lets newer readers
//! accept an older name; it does not make older readers accept a new name.
//! Tuple structs and tuple variants are positional, so appending fields also
//! requires compatibility in both directions. `#[serde(deny_unknown_fields)]`
//! must never be added to a dumped type.

use std::{convert::Infallible, error::Error, fmt, mem::size_of};

use minicbor_serde::{
    Deserializer, Serializer,
    error::{DecodeError, EncodeError},
};
use monty_types::{ModuleStub, TypeCheckState};
use serde::{Deserialize, Serialize, de::Error as _};

use crate::{
    dump_upgrade,
    heap::Heap,
    intern::Interns,
    repl::{MontyRepl, ReplProgress},
    run_progress::RunProgress,
};

/// Prefix distinguishing Monty dumps from unframed CBOR data.
const MAGIC: &[u8; 6] = b"MONTY\0";

/// Version of the dump schema.
///
/// Bump for the first incompatible change since the last published release, even
/// for a patch release. Further bumps before the next release are allowed but
/// not required. Check both directions: reading older dumps does not make a new
/// encoding readable by older releases.
///
/// Changes to the *meaning* of serialized data can also break compatibility:
/// opcodes or their operand shapes, `BuiltinsFunctions` discriminants (bytecode
/// operands), `CmpOperator` values,
/// the compiler's constant layout, numeric identities, how a dict or set key
/// hashes (entries persist their hash), or a semantic change to a stored value.
/// Frozen code is persisted too: incompatible changes to its Rust entry arguments or
/// private native helper contracts need compatibility handling or a bump. Compatible
/// source-body changes do not require one; restored sessions retain their captured code.
/// Older releases reject dumps with newer versions, and this build rejects dumps
/// below [`MIN_SUPPORTED_DUMP_VERSION`].
pub const DUMP_VERSION: u16 = 18;

/// The oldest dump version this build loads. `Dump::load` runs `dump_upgrade`
/// over an older dump, rebuilding what the newer versions changed.
pub const MIN_SUPPORTED_DUMP_VERSION: u16 = 13;

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
    module_stubs: &[ModuleStub],
    state: SessionRef<'_>,
) -> Result<Vec<u8>, DumpEncodeError> {
    /// Borrowed mirror of [`Dump`]; serde encodes it identically.
    #[derive(Serialize)]
    struct DumpRef<'a> {
        script_name: &'a str,
        type_check: Option<&'a TypeCheckState>,
        module_stubs: &'a [ModuleStub],
        state: SessionRef<'a>,
    }

    let mut bytes = Vec::with_capacity(HEADER_LEN + MIN_PAYLOAD_CAPACITY);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&DUMP_VERSION.to_le_bytes());
    // the payload is written after the header in place: no second buffer to copy it into
    let dump = DumpRef {
        script_name,
        type_check,
        module_stubs,
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
/// `type_check` disables enforcement the client asked for.
#[derive(Debug, Deserialize)]
pub struct Dump {
    /// Script name used for tracebacks and type-check diagnostics.
    pub script_name: String,
    /// `Some` when the session was created with type checking enabled.
    pub type_check: Option<TypeCheckState>,
    /// The stubs of the session's host-provided modules, whether or not it
    /// type-checks, so a restored session checks against the same ones.
    #[serde(default)]
    pub module_stubs: Vec<ModuleStub>,
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
    /// Accepts [`MIN_SUPPORTED_DUMP_VERSION`]`..=`[`DUMP_VERSION`], upgrading a
    /// dump older than [`DUMP_VERSION`] before returning it.
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
            let mut value =
                Self::deserialize(&mut deserializer).map_err(|err| DumpError::Payload(DumpDecodeError(err)))?;
            // trailing bytes are rejected rather than ignored, so a padded dump
            // cannot decode as the shorter valid one it starts with
            if deserializer.into_decoder().position() == payload.len() {
                if version < DUMP_VERSION
                    && let Some((heap, interns)) = value.state.heap_and_interns()
                {
                    dump_upgrade::upgrade(version, heap, interns);
                }
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

impl Session {
    /// The session heap and its interns, or `None` for a one-shot run that has completed.
    fn heap_and_interns(&mut self) -> Option<(&mut Heap, &mut Interns)> {
        match self {
            Self::Idle(repl) => Some(repl.heap_and_interns()),
            Self::Suspended(progress) => Some(progress.heap_and_interns()),
            Self::Running(progress) => progress.heap_and_interns(),
        }
    }
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

    use monty_types::{ExcType, MontyType, TypeCheckingFormat};
    use serde::Serialize;
    use strum::{IntoEnumIterator, VariantNames};

    use super::DUMP_VERSION;
    use crate::{bytecode::opcode_operand_shapes_fingerprint, expressions::CmpOperator, types::Type};

    /// Flags changes to persisted components for review. Before updating a fingerprint,
    /// check compatibility in both directions and bump the dump version if required
    /// by the policy on `DUMP_VERSION`.
    #[test]
    fn serialized_components_match_dump_version() {
        let operand_shapes = opcode_operand_shapes_fingerprint();
        assert_eq!(
            operand_shapes,
            0x19e4_7bfc_4a4e_4af1,
            "opcode operand shapes changed for dump version {DUMP_VERSION}, actual: {}",
            grouped_hex(operand_shapes)
        );

        let comparison_names: Vec<String> = CmpOperator::iter()
            .map(|op| encoded_variant_name(&minicbor_serde::to_vec(op).unwrap()))
            .collect();
        assert_eq!(
            variant_name_fingerprint(&comparison_names),
            0x763a_c25e_b7af_0536,
            "CmpOperator variants changed for dump version {DUMP_VERSION}, actual: {}",
            grouped_hex(variant_name_fingerprint(&comparison_names))
        );
        assert!(MontyType::VARIANTS.contains(&"exception"));

        let type_names: Vec<String> = Type::ALL
            .iter()
            .map(|ty| encoded_variant_name(&minicbor_serde::to_vec(ty).unwrap()))
            .collect();
        assert_eq!(
            variant_name_fingerprint(&type_names),
            0x9ad0_e4fb_60c6_4bc7,
            "Type variants changed for dump version {DUMP_VERSION}, actual: {}",
            grouped_hex(variant_name_fingerprint(&type_names))
        );
        let monty_type_names = serde_variant_names(MontyType::VARIANTS, &[MontyType::Exception(ExcType::ValueError)]);
        assert_eq!(
            variant_name_fingerprint(&monty_type_names),
            0x4791_e0b4_5926_cccd,
            "MontyType variants changed for dump version {DUMP_VERSION}, actual: {}",
            grouped_hex(variant_name_fingerprint(&monty_type_names))
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

    /// FNV-1a over variant names in sorted order.
    ///
    /// Reordering is compatible for enums encoded by name, but older readers may reject a new variant.
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

    /// Pins the serialized names of `TypeCheckingFormat` in `monty_types::TypeCheckState`.
    /// An older reader may reject a new variant; check both directions before updating the list.
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
