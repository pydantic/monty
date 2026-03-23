mod db;
mod stubs;
mod type_check;

pub use crate::{
    stubs::build_input_stubs,
    type_check::{SourceFile, TypeCheckingDiagnostics, type_check},
};
