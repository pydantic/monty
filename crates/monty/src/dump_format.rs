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
use serde::{Deserialize, Serialize};

use crate::{
    repl::{MontyRepl, ReplProgress},
    run_progress::RunProgress,
};

/// Prefix distinguishing Monty dumps from unframed postcard data.
const MAGIC: &[u8; 6] = b"MONTY\0";

/// Version of the dump's postcard schema.
///
/// Bump this whenever a serialized discriminant can shift, so older dumps are
/// rejected instead of decoding as their neighbour. That covers the
/// interpreter's own types *and* everything reachable from [`Dump`] — notably
/// `TypeCheckingConfig` in `monty-types`.
pub const DUMP_VERSION: u16 = 6;

/// Number of bytes before the postcard payload.
const HEADER_LEN: usize = MAGIC.len() + size_of::<u16>();

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

    let payload = postcard::to_allocvec(&DumpRef {
        script_name,
        type_check,
        state,
    })?;
    let mut bytes = Vec::with_capacity(HEADER_LEN + payload.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&DUMP_VERSION.to_le_bytes());
    bytes.extend_from_slice(&payload);
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
    /// # Errors
    /// Returns [`DumpError`] for a dump this build cannot read — most usefully
    /// [`DumpError::VersionMismatch`], which names both versions so a host can
    /// tell a stale snapshot from a corrupt one.
    pub fn load(bytes: &[u8]) -> Result<Self, DumpError> {
        let Some(header) = bytes.get(..HEADER_LEN) else {
            return Err(DumpError::NotADump);
        };
        let version = u16::from_le_bytes([header[MAGIC.len()], header[MAGIC.len() + 1]]);
        if &header[..MAGIC.len()] != MAGIC {
            Err(DumpError::NotADump)
        } else if version != DUMP_VERSION {
            Err(DumpError::VersionMismatch {
                found: version,
                expected: DUMP_VERSION,
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
/// Distinguishes the three failures a host cares about, because they need
/// different responses: an old snapshot should be discarded and rebuilt, while
/// a payload error on a current-version dump means corruption.
#[derive(Debug, PartialEq, Eq)]
pub enum DumpError {
    /// Too short to hold a header, or missing the magic prefix.
    NotADump,
    /// Written by a build using a different dump format version.
    VersionMismatch {
        /// Version the dump was written with.
        found: u16,
        /// Version this build reads.
        expected: u16,
    },
    /// Header was valid but the postcard payload did not decode.
    Payload(postcard::Error),
}

impl fmt::Display for DumpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotADump => write!(f, "not a monty dump"),
            Self::VersionMismatch { found, expected } => {
                write!(f, "dump format version {found}, this build reads {expected}")
            }
            Self::Payload(err) => write!(f, "malformed dump payload: {err}"),
        }
    }
}

impl Error for DumpError {}

#[cfg(test)]
mod tests {
    use monty_types::TypeCheckingFormat;
    use strum::VariantNames;

    use super::DUMP_VERSION;
    use crate::{
        bytecode::opcode_fingerprint, expressions::comparison_operators_fingerprint, intern::static_strings_fingerprint,
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
            0x0d57_34dd_be07_19ac,
            "opcodes changed for dump version {DUMP_VERSION}"
        );
        assert_eq!(
            static_strings_fingerprint(),
            0x239c_c721_30be_eba3,
            "static strings changed for dump version {DUMP_VERSION}"
        );
        assert_eq!(
            comparison_operators_fingerprint(),
            0x8ecc_d26b_160d_9c0b,
            "comparison operators changed for dump version {DUMP_VERSION}"
        );
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
