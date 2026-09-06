#![doc = include_str!("../README.md")]

#[cfg(all(feature = "ty", feature = "pyrefly"))]
compile_error!("the `ty` and `pyrefly` features are mutually exclusive.");

#[cfg(not(any(feature = "ty", feature = "pyrefly")))]
compile_error!("monty-type-checking needs exactly one backend: enable either the `ty` or `pyrefly` feature.");

#[cfg(feature = "ty")]
mod db;
#[cfg(feature = "pyrefly")]
mod pyrefly_check;
mod source_file;
#[cfg(feature = "ty")]
mod type_check;

#[cfg(feature = "pyrefly")]
pub use crate::pyrefly_check::{PyreflyChecker as TypeChecker, PyreflyDiagnostics as TypeCheckingDiagnostics};
pub use crate::source_file::SourceFile;
#[cfg(feature = "ty")]
pub use crate::type_check::{TypeChecker, TypeCheckingDiagnostics};
