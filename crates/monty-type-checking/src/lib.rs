#![doc = include_str!("../README.md")]

#[cfg(not(feature = "pyrefly"))]
mod db;
#[cfg(feature = "pyrefly")]
mod pyrefly_check;
mod source_file;
#[cfg(not(feature = "pyrefly"))]
mod type_check;

#[cfg(feature = "pyrefly")]
pub use crate::pyrefly_check::{PyreflyChecker as TypeChecker, PyreflyDiagnostics as TypeCheckingDiagnostics};
pub use crate::source_file::SourceFile;
#[cfg(not(feature = "pyrefly"))]
pub use crate::type_check::{TypeChecker, TypeCheckingDiagnostics};
