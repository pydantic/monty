//! `TypeCheckingConfig` ↔ `pb::Configure`'s type-check rendering fields.
//!
//! Type checking runs in the child, which renders the diagnostics before they
//! cross the wire (ty's structured diagnostics borrow the checker's database).
//! The parent therefore chooses the rendering up front, on `Configure`.

use monty_types::{ModuleStub, MontyObject, TypeCheckingConfig, TypeCheckingFormat};

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

/// Validates a dict of dotted module paths to `.pyi` source or `None`.
/// Paths must belong to a requested module; errors are protocol-violation messages.
pub fn import_stubs_from_answer(answer: &MontyObject, asked: &[String]) -> Result<Vec<ModuleStub>, String> {
    let answer = answer.as_ref();
    let pairs = match answer.pairs() {
        Some(pairs) if answer.type_name() == "dict" => pairs,
        _ => return Err("import_stubs must be answered with a dict of module to stub source".to_owned()),
    };
    let mut stubs = Vec::new();
    for (key, value) in pairs {
        let Some(module) = key.as_str() else {
            return Err("import_stubs answer keys must be module names".to_owned());
        };
        let top_level = module.split('.').next().unwrap_or_default();
        if !asked.iter().any(|name| name == top_level) {
            return Err(format!("import_stubs answered for {module:?}, which was not asked for"));
        }
        if let Some(source) = value.as_str() {
            stubs.push(ModuleStub::new(module, source).map_err(|err| format!("import_stubs: {err}"))?);
        } else if value.type_name() != "NoneType" {
            return Err(format!("import_stubs answer for {module:?} must be a str or None"));
        }
    }
    Ok(stubs)
}
