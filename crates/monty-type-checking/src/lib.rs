#![doc = include_str!("../README.md")]

#[cfg(feature = "checker")]
mod db;
mod imports;
#[cfg(feature = "checker")]
mod type_check;

pub use crate::imports::{ImportedModule, SnippetImports, scan_imports};
#[cfg(feature = "checker")]
pub use crate::type_check::{SourceFile, TypeCheckContext, TypeChecker, TypeCheckingDiagnostics};
