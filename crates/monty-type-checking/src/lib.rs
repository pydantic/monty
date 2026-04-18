mod db;
mod pool;
mod type_check;

pub use crate::type_check::{SourceFile, TypeCheckingDiagnostics, type_check};
