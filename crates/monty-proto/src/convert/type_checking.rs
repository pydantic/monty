//! `TypeCheckingConfig` ↔ `pb::Configure`'s type-check rendering fields, and
//! the module stubs `Configure` carries and `TypeStubs` reports.
//!
//! Type checking runs in the child, which renders the diagnostics before they
//! cross the wire (ty's structured diagnostics borrow the checker's database).
//! The parent therefore chooses the rendering up front, on `Configure`.

use std::collections::HashSet;

use monty_types::{ModuleStub, TypeCheckingConfig, TypeCheckingFormat};

use super::ProtoConvertError;
use crate::pb;

/// The module stubs a `Configure` or `TypeStubs` carries, validated as
/// [`ModuleStub`]s. A module named twice is refused too: it would be checked
/// against one stub and reported as both.
pub fn module_stubs_from_proto(stubs: &[pb::ModuleStub]) -> Result<Vec<ModuleStub>, ProtoConvertError> {
    let invalid = |reason: String| ProtoConvertError::InvalidValue {
        field: "ModuleStub.module",
        reason,
    };
    let mut seen = HashSet::with_capacity(stubs.len());
    stubs
        .iter()
        .map(|stub| {
            if !seen.insert(stub.module.as_str()) {
                return Err(invalid(format!("module {:?} has more than one stub", stub.module)));
            }
            ModuleStub::new(stub.module.clone(), stub.source.clone()).map_err(|err| invalid(err.to_string()))
        })
        .collect()
}

/// The wire form of `stubs`.
#[must_use]
pub fn module_stubs_to_proto(stubs: &[ModuleStub]) -> Vec<pb::ModuleStub> {
    stubs
        .iter()
        .map(|stub| pb::ModuleStub {
            module: stub.module().to_owned(),
            source: stub.source().to_owned(),
        })
        .collect()
}

impl From<TypeCheckingFormat> for pb::TypeCheckFormat {
    fn from(format: TypeCheckingFormat) -> Self {
        match format {
            TypeCheckingFormat::Full => Self::Full,
            TypeCheckingFormat::Concise => Self::Concise,
            TypeCheckingFormat::Azure => Self::Azure,
            TypeCheckingFormat::Json => Self::Json,
            TypeCheckingFormat::JsonLines => Self::JsonLines,
            TypeCheckingFormat::Rdjson => Self::Rdjson,
            TypeCheckingFormat::Pylint => Self::Pylint,
            TypeCheckingFormat::Gitlab => Self::Gitlab,
            TypeCheckingFormat::Github => Self::Github,
        }
    }
}

impl From<pb::TypeCheckFormat> for TypeCheckingFormat {
    /// `Unspecified` means an older parent that never set the field; it maps to
    /// the default (`Full`), which is what such a parent used to get.
    fn from(format: pb::TypeCheckFormat) -> Self {
        match format {
            pb::TypeCheckFormat::Unspecified | pb::TypeCheckFormat::Full => Self::Full,
            pb::TypeCheckFormat::Concise => Self::Concise,
            pb::TypeCheckFormat::Azure => Self::Azure,
            pb::TypeCheckFormat::Json => Self::Json,
            pb::TypeCheckFormat::JsonLines => Self::JsonLines,
            pb::TypeCheckFormat::Rdjson => Self::Rdjson,
            pb::TypeCheckFormat::Pylint => Self::Pylint,
            pb::TypeCheckFormat::Gitlab => Self::Gitlab,
            pb::TypeCheckFormat::Github => Self::Github,
        }
    }
}

impl From<&pb::Configure> for TypeCheckingConfig {
    /// An unrecognized format number (a peer built against a newer schema)
    /// falls back to the default rather than failing the session — the choice
    /// is cosmetic, and rejecting a whole checkout over it would be worse.
    fn from(configure: &pb::Configure) -> Self {
        Self {
            format: configure.type_check_format().into(),
            color: configure.type_check_color,
        }
    }
}
