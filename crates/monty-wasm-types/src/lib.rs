//! The semantic WIT types of Monty's WebAssembly components, and their
//! conversions to and from `monty-proto` messages.
//!
//! Two components share these types: `monty-wasm-runtime` (the interpreter
//! worker) and `monty-wasm-proto` (the protocol codec a browser uses to reach a
//! remote worker). This crate generates `pydantic:monty/types` once; each
//! component maps the interface onto [`types`] with wit-bindgen's `with`, so a
//! conversion written here serves both.

mod bindings {
    wit_bindgen::generate!({ world: "monty-types" });
}

mod event;
mod request;
mod value;

pub use bindings::pydantic::monty::types;
pub use event::{EventError, convert_event, push_print};
pub use request::request_from_component;
