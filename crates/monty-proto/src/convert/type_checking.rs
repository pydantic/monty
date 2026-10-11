//! `TypeCheckingConfig` ↔ `pb::Configure`'s type-check rendering fields.
//!
//! Type checking runs in the child, which renders the diagnostics before they
//! cross the wire (ty's structured diagnostics borrow the checker's database).
//! The parent therefore chooses the rendering up front, on `Configure`.

use monty_types::{TypeCheckingConfig, TypeCheckingFormat};

use crate::pb;

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
