//! Legacy numeric identity payloads, independent of the snapshot serializer.
//! Rust discriminants are permanent codes; serde's positional enum encoding does not honour them.

use super::bytes_payload;
use crate::{
    builtins::{Builtins, BuiltinsFunctions},
    modules::{ModuleFunctions, ModuleFunctionsDiscriminants, dataclasses::DataclassesFunctions},
    types::{Property, Type},
    value::Marker,
};

/// Maximum payload length before the sentinel and category tag exhaust `u128`.
const MAX_BYTES: usize = 14;

/// Preserves builtin categories from before exception classes and `type` joined the native type table.
pub(super) fn builtin(value: Builtins) -> u128 {
    match value {
        Builtins::Function(function) => indices_payload(&[0, function as u32]),
        Builtins::Type(Type::Type) => indices_payload(&[0, BuiltinsFunctions::Type as u32]),
        Builtins::Type(ty) => match ty.exc_type() {
            Some(exception) => indices_payload(&[1, exception as u32]),
            None => indices_payload(&[2, ty.legacy_index()]),
        },
    }
}

/// Preserves module and function indices, including configured decorator options.
pub(super) fn module_function(value: ModuleFunctions) -> u128 {
    let module = ModuleFunctionsDiscriminants::from(value) as u32;
    let function = match value {
        ModuleFunctions::Asyncio(function) => function as u32,
        ModuleFunctions::Collections(function) => function as u32,
        ModuleFunctions::Json(function) => function as u32,
        ModuleFunctions::Math(function) => function as u32,
        ModuleFunctions::Os(function) => function as u32,
        ModuleFunctions::OsPath(function) => function as u32,
        ModuleFunctions::Re(function) => function as u32,
        ModuleFunctions::Unicodedata(function) => function as u32,
        ModuleFunctions::Itertools(function) => function as u32,
        ModuleFunctions::Dataclasses(function) => {
            return match function {
                DataclassesFunctions::Dataclass => indices_payload(&[module, 0]),
                DataclassesFunctions::IsDataclass => indices_payload(&[module, 1]),
                DataclassesFunctions::Field => indices_payload(&[module, 2]),
                DataclassesFunctions::Configured(options) => {
                    indices_payload(&[module, 3, u32::from(options.eq), u32::from(options.frozen)])
                }
            };
        }
        ModuleFunctions::Functools(function) => function as u32,
        ModuleFunctions::Base64(function) => function as u32,
        ModuleFunctions::Binascii(function) => function as u32,
        ModuleFunctions::Random(function) => function as u32,
        ModuleFunctions::Copy(function) => function as u32,
        ModuleFunctions::Frozen(function) => function as u32,
        ModuleFunctions::Time(function) => function as u32,
        ModuleFunctions::Hashlib(function) => function as u32,
        ModuleFunctions::Bisect(function) => function as u32,
        ModuleFunctions::Csv(function) => function as u32,
        ModuleFunctions::Tomllib(function) => function as u32,
        ModuleFunctions::Decimal(function) => function as u32,
        ModuleFunctions::Fnmatch(function) => function as u32,
        ModuleFunctions::Typing(function) => function as u32,
        ModuleFunctions::Annotationlib(function) => function as u32,
        ModuleFunctions::Statistics(function) => function as u32,
        #[cfg(feature = "test-hooks")]
        ModuleFunctions::Gc(function) => function as u32,
        #[cfg(feature = "test-hooks")]
        ModuleFunctions::Sys(function) => function as u32,
    };
    indices_payload(&[module, function])
}

/// Preserves the length-prefixed UTF-8 name used for stateless marker identities.
pub(super) fn marker(value: Marker) -> u128 {
    let name: &'static str = value.0.into();
    let mut bytes = [0; MAX_BYTES];
    bytes[0] = u8::try_from(name.len()).expect("marker name length fits u8");
    bytes[1..=name.len()].copy_from_slice(name.as_bytes());
    bytes_payload(&bytes[..=name.len()])
}

/// Preserves the property category and its zero-argument OS operation index.
pub(super) fn property(value: Property) -> u128 {
    match value {
        Property::Os(operation) => indices_payload(&[0, operation as u32]),
    }
}

/// Encodes unsigned LEB128 indices with the same byte order and sentinel as legacy IDs.
fn indices_payload(indices: &[u32]) -> u128 {
    let mut bytes = [0; MAX_BYTES];
    let mut len = 0;
    for &index in indices {
        let mut remaining = index;
        loop {
            let byte = u8::try_from(remaining & 0x7f).expect("seven bits fit u8");
            remaining >>= 7;
            bytes[len] = byte | if remaining == 0 { 0 } else { 0x80 };
            len += 1;
            if remaining == 0 {
                break;
            }
        }
    }
    bytes_payload(&bytes[..len])
}
