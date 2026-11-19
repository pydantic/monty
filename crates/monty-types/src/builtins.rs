//! [`BuiltinsFunctions`] — the name-level identity of every interpreter-native
//! Python builtin, carried by [`MontyObject::builtin_function`](crate::MontyObject::builtin_function).

use strum::{Display, EnumString, FromRepr, IntoStaticStr, VariantNames};
/// Enumerates every interpreter-native Python builtin function.
///
/// Listed alphabetically per <https://docs.python.org/3/library/functions.html>
/// Commented-out variants are not yet implemented.
///
/// Note: Type constructors are handled by the `Type` enum, not here.
///
/// Uses strum derives for automatic `Display`, `FromStr`, and `IntoStaticStr` implementations.
/// All variants serialize to lowercase (e.g., `Print` -> "print").
/// Explicit discriminants are bytecode operands and identity codes; never change or reuse them.
#[monty_macros::explicit_discriminants(checksum = 0x363c_1f03_edf2_19af)]
#[derive(
    Debug,
    Clone,
    Copy,
    Display,
    EnumString,
    FromRepr,
    IntoStaticStr,
    VariantNames,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
// `use_phf` while still below the ~100-variant threshold: it breaks even now and the enum keeps growing.
#[strum(serialize_all = "lowercase", use_phf)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum BuiltinsFunctions {
    Abs = 0,
    // Aiter,
    All = 1,
    // Anext,
    Any = 2,
    // Ascii,
    Bin = 3,
    // bool - handled by Type enum
    // Breakpoint,
    // bytearray - handled by Type enum
    // bytes - handled by Type enum
    // Callable,
    Chr = 4,
    // Classmethod,
    // Compile,
    // complex - handled by Type enum
    // Delattr,
    // dict - handled by Type enum
    // Dir,
    Divmod = 5,
    Enumerate = 6,
    Filter = 7,
    // float - handled by Type enum
    // Format - appended below
    // frozenset - handled by Type enum
    Getattr = 8,
    // Globals,
    Hasattr = 9,
    Hash = 10,
    // Help,
    Hex = 11,
    Id = 12,
    // Input,
    // int - handled by Type enum
    Isinstance = 13,
    // Iter - handled by Type enum
    Len = 14,
    // list - handled by Type enum
    Map = 15,
    Max = 16,
    // memoryview - handled by Type enum
    Min = 17,
    Next = 18,
    // object - handled by Type enum
    Oct = 19,
    Open = 20,
    Ord = 21,
    Pow = 22,
    Print = 23,
    // Property,
    // range - handled by Type enum
    Repr = 24,
    Reversed = 25,
    Round = 26,
    // set - handled by Type enum
    Setattr = 27,
    // Slice,
    Sorted = 28,
    // Staticmethod,
    // str - handled by Type enum
    Sum = 29,
    // super - handled by Type enum
    // tuple - handled by Type enum
    Type = 30,
    // Vars,
    Zip = 31,
    // __import__ - not planned
    // These codes were assigned after the original alphabetical block.
    /// `object.__setattr__(obj, name, value)` — the write that bypasses a
    /// class's attribute hooks, reached only through `object`. Its name is not
    /// an identifier, so it can never resolve as a bare global.
    ///
    /// The first variant whose name is not its lowercased identifier, so it
    /// needs both renames: serde and strum each carry the name across a
    /// different boundary (JSON vs. `Display`/`FromStr`) and must agree.
    #[strum(serialize = "object.__setattr__")]
    #[serde(rename = "object.__setattr__")]
    ObjectSetattr = 32,
    /// `format(value, format_spec='')`, appended after [`Self::ObjectSetattr`].
    Format = 33,
    /// `eval(source, /, globals=None, locals=None)`, appended after [`Self::Format`].
    Eval = 34,
    /// `exec(source, /, globals=None, locals=None, *, closure=None)`, appended after [`Self::Eval`].
    Exec = 35,
    /// `locals()`, appended after [`Self::Exec`].
    Locals = 36,
    /// `issubclass(class, classinfo)`, appended after [`Self::Locals`].
    Issubclass = 37,
    /// `object.__new__(cls, /, *args, **kwargs)`, reached through `object` or `super()`.
    #[strum(serialize = "object.__new__")]
    #[serde(rename = "object.__new__")]
    ObjectNew = 38,
    /// `object.__init__(self, /, *args, **kwargs)`, reached through `object` or `super()`.
    #[strum(serialize = "object.__init__")]
    #[serde(rename = "object.__init__")]
    ObjectInit = 39,
    /// `object.__init_subclass__(cls, /, **kwargs)`, run after a class statement.
    #[strum(serialize = "object.__init_subclass__")]
    #[serde(rename = "object.__init_subclass__")]
    ObjectInitSubclass = 40,
    /// `type.mro(cls, /)`, a class's method resolution order as a list.
    #[strum(serialize = "type.mro")]
    #[serde(rename = "type.mro")]
    TypeMro = 41,
}
