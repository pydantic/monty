#![doc = include_str!("../README.md")]

mod db;
mod type_check;

pub use crate::type_check::{SourceFile, TypeChecker, TypeCheckingDiagnostics};
