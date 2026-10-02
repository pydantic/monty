#![doc = include_str!("../README.md")]

mod db;
mod imports;
mod type_check;

pub use crate::{
    imports::top_level_imports,
    type_check::{SourceFile, TypeCheckContext, TypeChecker, TypeCheckingDiagnostics},
};
