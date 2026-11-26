//! `TypeCheckingConfig` ↔ `pb::Configure`'s type-check rendering fields.
//!
//! Type checking runs in the worker, which renders the diagnostics before they
//! cross the wire (ty's structured diagnostics borrow the checker's database).
//! The client therefore chooses the rendering up front, on `Configure`.

use monty_types::{
    ModuleStub, MontyObject, TypeCheckingConfig, TypeCheckingFormat,
    unstable::{MontyNode, graph_parts},
};

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
    /// `Unspecified` means an older client that never set the field; it maps to
    /// the default (`Full`), which is what such a client used to get.
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

// This per-reply estimate is independent of the encoded frame size and total checker memory.
const MAX_EXPANDED_STUB_BYTES: usize = 64 * 1024 * 1024;

/// Validates an import-stubs reply without copying its sources.
/// Each reply has a 64 MiB estimated budget for names, sources, generated paths and entry storage.
/// Counts shared strings and ancestor paths per entry; excludes `None` entries.
/// This is not a limit on total type-checker memory.
pub fn validate_import_stubs_answer(answer: &MontyObject, asked: &[String]) -> Result<(), String> {
    validated_stub_pairs(answer, asked).map(|_| ())
}

/// Validates a dict of dotted module paths to `.pyi` source or `None`.
/// Paths must belong to a requested module; errors are protocol-violation messages.
/// Validates the entire reply before copying sources into owned module stubs.
/// Uses the same estimated storage budget as [`validate_import_stubs_answer`].
pub fn import_stubs_from_answer(answer: &MontyObject, asked: &[String]) -> Result<Vec<ModuleStub>, String> {
    validated_stub_pairs(answer, asked)?
        .into_iter()
        .map(|(module, source)| ModuleStub::new(module, source).map_err(|err| format!("import_stubs: {err}")))
        .collect()
}

fn validated_stub_pairs<'a>(answer: &'a MontyObject, asked: &[String]) -> Result<Vec<(&'a str, &'a str)>, String> {
    let (graph, root) = graph_parts(answer);
    let MontyNode::Dict(pairs) = graph.node(root) else {
        return Err("import_stubs must be answered with a dict of module to stub source".to_owned());
    };
    let mut stubs = Vec::new();
    let mut remaining = MAX_EXPANDED_STUB_BYTES;
    for &(key, value) in pairs {
        let key = graph.value(key);
        let value = graph.value(value);
        let Some(module) = key.as_str() else {
            return Err("import_stubs answer keys must be module names".to_owned());
        };
        let top_level = module.split('.').next().unwrap_or_default();
        if !asked.iter().any(|name| name == top_level) {
            return Err(format!("import_stubs answered for {module:?}, which was not asked for"));
        }
        if let Some(source) = value.as_str() {
            remaining = remaining
                .checked_sub(module.len())
                .and_then(|bytes| bytes.checked_sub(source.len()))
                // Both vectors can have spare capacity while borrowed pairs become owned stubs.
                .and_then(|bytes| bytes.checked_sub(2 * (size_of::<(&str, &str)>() + size_of::<ModuleStub>())))
                .ok_or_else(stub_budget_exceeded)?;
            // A dot's byte offset is the ancestor path length after replacing dots with slashes.
            // Charge every ancestor per entry, even when another stub supplies or shares it.
            // Treat the module itself as a package, whose suffix is longer than `.pyi`.
            for path_len in module
                .match_indices('.')
                .map(|(offset, _)| offset)
                .chain([module.len()])
            {
                remaining = remaining
                    .checked_sub(path_len)
                    .and_then(|bytes| bytes.checked_sub("/__init__.pyi".len()))
                    .and_then(|bytes| bytes.checked_sub(2 * size_of::<(String, &str)>()))
                    .ok_or_else(stub_budget_exceeded)?;
            }
            ModuleStub::validate_module(module).map_err(|err| format!("import_stubs: {err}"))?;
            stubs.push((module, source));
        } else if value.type_name() != "NoneType" {
            return Err(format!("import_stubs answer for {module:?} must be a str or None"));
        }
    }
    Ok(stubs)
}

/// Reports exhaustion of the per-reply estimate before copying sources or constructing file paths.
fn stub_budget_exceeded() -> String {
    "import_stubs estimated expanded storage exceeds 64 MiB".to_owned()
}
