//! [`BuiltinsFunctions`] — the name-level identity of every interpreter-native
//! Python builtin, carried by [`MontyObject::BuiltinFunction`](crate::object::MontyObject).

use strum::{Display, EnumString, FromRepr, IntoStaticStr};
/// Enumerates every interpreter-native Python builtin function.
///
/// Listed alphabetically per <https://docs.python.org/3/library/functions.html>
/// Commented-out variants are not yet implemented.
///
/// Note: Type constructors are handled by the `Type` enum, not here.
///
/// Uses strum derives for automatic `Display`, `FromStr`, and `IntoStaticStr` implementations.
/// All variants serialize to lowercase (e.g., `Print` -> "print").
#[derive(
    Debug,
    Clone,
    Copy,
    Display,
    EnumString,
    FromRepr,
    IntoStaticStr,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[strum(serialize_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum BuiltinsFunctions {
    Abs,
    // Aiter,
    All,
    // Anext,
    Any,
    // Ascii,
    Bin,
    // bool - handled by Type enum
    // Breakpoint,
    // bytearray - handled by Type enum
    // bytes - handled by Type enum
    // Callable,
    Chr,
    // Classmethod,
    // Compile,
    // complex - handled by Type enum
    // Delattr,
    // dict - handled by Type enum
    // Dir,
    Divmod,
    Enumerate,
    // Eval,
    // Exec,
    Filter,
    // float - handled by Type enum
    // Format,
    // frozenset - handled by Type enum
    Getattr,
    // Globals,
    Hasattr,
    Hash,
    // Help,
    Hex,
    Id,
    // Input,
    // int - handled by Type enum
    Isinstance,
    // Issubclass,
    // Iter - handled by Type enum
    Len,
    // list - handled by Type enum
    // Locals,
    Map,
    Max,
    // memoryview - handled by Type enum
    Min,
    Next,
    // object - handled by Type enum
    Oct,
    Open,
    Ord,
    Pow,
    Print,
    // Property,
    // range - handled by Type enum
    Repr,
    Reversed,
    Round,
    // set - handled by Type enum
    Setattr,
    // Slice,
    Sorted,
    // Staticmethod,
    // str - handled by Type enum
    Sum,
    // Super,
    // tuple - handled by Type enum
    Type,
    // Vars,
    Zip,
    // __import__ - not planned
}
