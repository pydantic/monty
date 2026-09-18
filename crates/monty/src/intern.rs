//! String, bytes, and long integer interning for efficient storage of literals and identifiers.
//!
//! This module provides interners that store unique strings, bytes, and long integers in vectors
//! and return indices (`StringId`, `BytesId`, `LongIntId`) for efficient storage and comparison.
//! This avoids the overhead of cloning strings or using atomic reference counting.
//!
//! One table serves parsing, preparation, compilation and execution. Runtime paths
//! can append static strings without invalidating existing borrows.
//!
//! StringIds are laid out as follows:
//! * 0 to 127 - single character strings for all 128 ASCII characters
//! * 128 - the empty string
//! * 129+ - strings interned per executor
//!
//! Other static strings occupy ordinary executor-local slots. Their interner entries
//! retain a [`StaticStrings`] tag for dispatch, while snapshots serialize only
//! their text so another build can load an unknown static string as owned text.

use std::{cell::RefCell, mem, ops::Index, slice::from_ref, str::FromStr, sync::LazyLock};

use ahash::AHashMap;
use num_bigint::BigInt;
use strum::{EnumString, FromRepr, IntoStaticStr};

use crate::{
    function::Function,
    hash::{HashValue, RESERVED_STRING_HASHES, WithHash, hash_python_str},
    heap::{HeapId, StableHeap},
};

/// Index into the string interner's storage.
///
/// Uses `u32` to save space (4 bytes vs 8 bytes for `usize`). This limits us to
/// ~4 billion unique interns, which is more than sufficient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub struct StringId(u32);

impl StringId {
    /// Executor-independent ID for the empty string, immediately after ASCII.
    pub const EMPTY: Self = Self(128);

    /// Creates a StringId from a raw index value.
    ///
    /// Used by the bytecode VM to reconstruct StringIds from operands stored
    /// in bytecode. The caller is responsible for ensuring the index is valid.
    #[inline]
    pub fn from_index(index: u16) -> Self {
        Self(u32::from(index))
    }

    /// Returns the raw index value.
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }

    /// Returns the StringId for an ASCII byte.
    #[must_use]
    pub const fn from_ascii(byte: u8) -> Self {
        Self(byte as u32)
    }
}

/// Executor-local intern IDs follow ASCII and the empty string.
const INTERN_STRING_ID_OFFSET: usize = RESERVED_STRS.len();

/// Strings runtime paths can materialize without a corresponding source name.
const CORE_STATIC_STRINGS: &[StaticStrings] = &[
    StaticStrings::Module,
    StaticStrings::NoneRepr,
    StaticStrings::TrueRepr,
    StaticStrings::FalseRepr,
    StaticStrings::EllipsisRepr,
    StaticStrings::NotImplementedRepr,
    StaticStrings::DunderMain,
    StaticStrings::DunderDoc,
];

/// Executor-independent text for ASCII IDs 0–127 and the empty-string ID 128.
/// Hashes in [`crate::hash::RESERVED_STRING_HASHES`] use the same indices.
pub(crate) static RESERVED_STRS: [&str; 129] = const {
    // Initialize array of 128 bytes which will be used as the raw storage
    const ASCII_BYTES: [u8; 128] = const {
        let mut bytes: [u8; 128] = [0; 128];
        let mut i: u8 = 0;
        while i < 128 {
            bytes[i as usize] = i;
            i += 1;
        }
        bytes
    };
    // Index into the above array to build the `&'static str` forms
    let mut strs: [&str; 129] = [""; 129];
    let mut i = 0;
    while i < 128 {
        strs[i] = match str::from_utf8(from_ref(&ASCII_BYTES[i])) {
            Ok(s) => s,
            Err(_) => panic!("invalid ascii byte"),
        };
        i += 1;
    }
    strs
};

/// Static string values known at compile time.
///
/// The `Ascii*` variants and [`Self::EmptyString`] pin the discriminants
/// [`get_static_string`] reverses with [`FromRepr`](strum::FromRepr), so they
/// stay in code-point order. Every variant after them is in alphabetical
/// order by variant name, purely so that two branches adding a string rarely
/// touch the same line; nothing reads those discriminants. Interner entries
/// serialize as text and recover a tag only when the loading build recognizes
/// that text, so reordering them does not invalidate a dump.
#[repr(u16)]
#[derive(Debug, Clone, Copy, EnumString, FromRepr, IntoStaticStr, PartialEq, Eq, Hash)]
#[strum(serialize_all = "snake_case")]
pub enum StaticStrings {
    /// ASCII character 0x00.
    #[strum(serialize = "\x00")]
    AsciiNull = 0,
    /// ASCII character 0x01.
    #[strum(serialize = "\x01")]
    AsciiStartOfHeading = 1,
    /// ASCII character 0x02.
    #[strum(serialize = "\x02")]
    AsciiStartOfText = 2,
    /// ASCII character 0x03.
    #[strum(serialize = "\x03")]
    AsciiEndOfText = 3,
    /// ASCII character 0x04.
    #[strum(serialize = "\x04")]
    AsciiEndOfTransmission = 4,
    /// ASCII character 0x05.
    #[strum(serialize = "\x05")]
    AsciiEnquiry = 5,
    /// ASCII character 0x06.
    #[strum(serialize = "\x06")]
    AsciiAcknowledge = 6,
    /// ASCII character 0x07.
    #[strum(serialize = "\x07")]
    AsciiBell = 7,
    /// ASCII character 0x08.
    #[strum(serialize = "\x08")]
    AsciiBackspace = 8,
    /// ASCII character 0x09.
    #[strum(serialize = "\x09")]
    AsciiTab = 9,
    /// ASCII character 0x0a.
    #[strum(serialize = "\x0a")]
    AsciiLineFeed = 10,
    /// ASCII character 0x0b.
    #[strum(serialize = "\x0b")]
    AsciiVerticalTab = 11,
    /// ASCII character 0x0c.
    #[strum(serialize = "\x0c")]
    AsciiFormFeed = 12,
    /// ASCII character 0x0d.
    #[strum(serialize = "\x0d")]
    AsciiCarriageReturn = 13,
    /// ASCII character 0x0e.
    #[strum(serialize = "\x0e")]
    AsciiShiftOut = 14,
    /// ASCII character 0x0f.
    #[strum(serialize = "\x0f")]
    AsciiShiftIn = 15,
    /// ASCII character 0x10.
    #[strum(serialize = "\x10")]
    AsciiDataLinkEscape = 16,
    /// ASCII character 0x11.
    #[strum(serialize = "\x11")]
    AsciiDeviceControl1 = 17,
    /// ASCII character 0x12.
    #[strum(serialize = "\x12")]
    AsciiDeviceControl2 = 18,
    /// ASCII character 0x13.
    #[strum(serialize = "\x13")]
    AsciiDeviceControl3 = 19,
    /// ASCII character 0x14.
    #[strum(serialize = "\x14")]
    AsciiDeviceControl4 = 20,
    /// ASCII character 0x15.
    #[strum(serialize = "\x15")]
    AsciiNegativeAcknowledge = 21,
    /// ASCII character 0x16.
    #[strum(serialize = "\x16")]
    AsciiSynchronousIdle = 22,
    /// ASCII character 0x17.
    #[strum(serialize = "\x17")]
    AsciiEndOfTransmissionBlock = 23,
    /// ASCII character 0x18.
    #[strum(serialize = "\x18")]
    AsciiCancel = 24,
    /// ASCII character 0x19.
    #[strum(serialize = "\x19")]
    AsciiEndOfMedium = 25,
    /// ASCII character 0x1a.
    #[strum(serialize = "\x1a")]
    AsciiSubstitute = 26,
    /// ASCII character 0x1b.
    #[strum(serialize = "\x1b")]
    AsciiEscape = 27,
    /// ASCII character 0x1c.
    #[strum(serialize = "\x1c")]
    AsciiFileSeparator = 28,
    /// ASCII character 0x1d.
    #[strum(serialize = "\x1d")]
    AsciiGroupSeparator = 29,
    /// ASCII character 0x1e.
    #[strum(serialize = "\x1e")]
    AsciiRecordSeparator = 30,
    /// ASCII character 0x1f.
    #[strum(serialize = "\x1f")]
    AsciiUnitSeparator = 31,
    /// ASCII character 0x20.
    #[strum(serialize = "\x20")]
    AsciiSpace = 32,
    /// ASCII character 0x21.
    #[strum(serialize = "\x21")]
    AsciiExclamationMark = 33,
    /// ASCII character 0x22.
    #[strum(serialize = "\x22")]
    AsciiDoubleQuote = 34,
    /// ASCII character 0x23.
    #[strum(serialize = "\x23")]
    AsciiHash = 35,
    /// ASCII character 0x24.
    #[strum(serialize = "\x24")]
    AsciiDollar = 36,
    /// ASCII character 0x25.
    #[strum(serialize = "\x25")]
    AsciiPercent = 37,
    /// ASCII character 0x26.
    #[strum(serialize = "\x26")]
    AsciiAmpersand = 38,
    /// ASCII character 0x27.
    #[strum(serialize = "\x27")]
    AsciiSingleQuote = 39,
    /// ASCII character 0x28.
    #[strum(serialize = "\x28")]
    AsciiLeftParen = 40,
    /// ASCII character 0x29.
    #[strum(serialize = "\x29")]
    AsciiRightParen = 41,
    /// ASCII character 0x2a.
    #[strum(serialize = "\x2a")]
    AsciiAsterisk = 42,
    /// ASCII character 0x2b.
    #[strum(serialize = "\x2b")]
    AsciiPlus = 43,
    /// ASCII character 0x2c.
    #[strum(serialize = "\x2c")]
    AsciiComma = 44,
    /// ASCII character 0x2d.
    #[strum(serialize = "\x2d")]
    AsciiHyphen = 45,
    /// ASCII character 0x2e.
    #[strum(serialize = "\x2e")]
    AsciiDot = 46,
    /// ASCII character 0x2f.
    #[strum(serialize = "\x2f")]
    AsciiSlash = 47,
    /// ASCII character 0x30.
    #[strum(serialize = "\x30")]
    AsciiDigit0 = 48,
    /// ASCII character 0x31.
    #[strum(serialize = "\x31")]
    AsciiDigit1 = 49,
    /// ASCII character 0x32.
    #[strum(serialize = "\x32")]
    AsciiDigit2 = 50,
    /// ASCII character 0x33.
    #[strum(serialize = "\x33")]
    AsciiDigit3 = 51,
    /// ASCII character 0x34.
    #[strum(serialize = "\x34")]
    AsciiDigit4 = 52,
    /// ASCII character 0x35.
    #[strum(serialize = "\x35")]
    AsciiDigit5 = 53,
    /// ASCII character 0x36.
    #[strum(serialize = "\x36")]
    AsciiDigit6 = 54,
    /// ASCII character 0x37.
    #[strum(serialize = "\x37")]
    AsciiDigit7 = 55,
    /// ASCII character 0x38.
    #[strum(serialize = "\x38")]
    AsciiDigit8 = 56,
    /// ASCII character 0x39.
    #[strum(serialize = "\x39")]
    AsciiDigit9 = 57,
    /// ASCII character 0x3a.
    #[strum(serialize = "\x3a")]
    AsciiColon = 58,
    /// ASCII character 0x3b.
    #[strum(serialize = "\x3b")]
    AsciiSemicolon = 59,
    /// ASCII character 0x3c.
    #[strum(serialize = "\x3c")]
    AsciiLessThan = 60,
    /// ASCII character 0x3d.
    #[strum(serialize = "\x3d")]
    AsciiEquals = 61,
    /// ASCII character 0x3e.
    #[strum(serialize = "\x3e")]
    AsciiGreaterThan = 62,
    /// ASCII character 0x3f.
    #[strum(serialize = "\x3f")]
    AsciiQuestionMark = 63,
    /// ASCII character 0x40.
    #[strum(serialize = "\x40")]
    AsciiAt = 64,
    /// ASCII character 0x41.
    #[strum(serialize = "\x41")]
    AsciiA = 65,
    /// ASCII character 0x42.
    #[strum(serialize = "\x42")]
    AsciiB = 66,
    /// ASCII character 0x43.
    #[strum(serialize = "\x43")]
    AsciiC = 67,
    /// ASCII character 0x44.
    #[strum(serialize = "\x44")]
    AsciiD = 68,
    /// ASCII character 0x45.
    #[strum(serialize = "\x45")]
    AsciiE = 69,
    /// ASCII character 0x46.
    #[strum(serialize = "\x46")]
    AsciiF = 70,
    /// ASCII character 0x47.
    #[strum(serialize = "\x47")]
    AsciiG = 71,
    /// ASCII character 0x48.
    #[strum(serialize = "\x48")]
    AsciiH = 72,
    /// ASCII character 0x49.
    #[strum(serialize = "\x49")]
    AsciiI = 73,
    /// ASCII character 0x4a.
    #[strum(serialize = "\x4a")]
    AsciiJ = 74,
    /// ASCII character 0x4b.
    #[strum(serialize = "\x4b")]
    AsciiK = 75,
    /// ASCII character 0x4c.
    #[strum(serialize = "\x4c")]
    AsciiL = 76,
    /// ASCII character 0x4d.
    #[strum(serialize = "\x4d")]
    AsciiM = 77,
    /// ASCII character 0x4e.
    #[strum(serialize = "\x4e")]
    AsciiN = 78,
    /// ASCII character 0x4f.
    #[strum(serialize = "\x4f")]
    AsciiO = 79,
    /// ASCII character 0x50.
    #[strum(serialize = "\x50")]
    AsciiP = 80,
    /// ASCII character 0x51.
    #[strum(serialize = "\x51")]
    AsciiQ = 81,
    /// ASCII character 0x52.
    #[strum(serialize = "\x52")]
    AsciiR = 82,
    /// ASCII character 0x53.
    #[strum(serialize = "\x53")]
    AsciiS = 83,
    /// ASCII character 0x54.
    #[strum(serialize = "\x54")]
    AsciiT = 84,
    /// ASCII character 0x55.
    #[strum(serialize = "\x55")]
    AsciiU = 85,
    /// ASCII character 0x56.
    #[strum(serialize = "\x56")]
    AsciiV = 86,
    /// ASCII character 0x57.
    #[strum(serialize = "\x57")]
    AsciiW = 87,
    /// ASCII character 0x58.
    #[strum(serialize = "\x58")]
    AsciiX = 88,
    /// ASCII character 0x59.
    #[strum(serialize = "\x59")]
    AsciiY = 89,
    /// ASCII character 0x5a.
    #[strum(serialize = "\x5a")]
    AsciiZ = 90,
    /// ASCII character 0x5b.
    #[strum(serialize = "\x5b")]
    AsciiLeftBracket = 91,
    /// ASCII character 0x5c.
    #[strum(serialize = "\x5c")]
    AsciiBackslash = 92,
    /// ASCII character 0x5d.
    #[strum(serialize = "\x5d")]
    AsciiRightBracket = 93,
    /// ASCII character 0x5e.
    #[strum(serialize = "\x5e")]
    AsciiCaret = 94,
    /// ASCII character 0x5f.
    #[strum(serialize = "\x5f")]
    AsciiUnderscore = 95,
    /// ASCII character 0x60.
    #[strum(serialize = "\x60")]
    AsciiBacktick = 96,
    /// ASCII character 0x61.
    #[strum(serialize = "\x61")]
    AsciiLowerA = 97,
    /// ASCII character 0x62.
    #[strum(serialize = "\x62")]
    AsciiLowerB = 98,
    /// ASCII character 0x63.
    #[strum(serialize = "\x63")]
    AsciiLowerC = 99,
    /// ASCII character 0x64.
    #[strum(serialize = "\x64")]
    AsciiLowerD = 100,
    /// ASCII character 0x65.
    #[strum(serialize = "\x65")]
    AsciiLowerE = 101,
    /// ASCII character 0x66.
    #[strum(serialize = "\x66")]
    AsciiLowerF = 102,
    /// ASCII character 0x67.
    #[strum(serialize = "\x67")]
    AsciiLowerG = 103,
    /// ASCII character 0x68.
    #[strum(serialize = "\x68")]
    AsciiLowerH = 104,
    /// ASCII character 0x69.
    #[strum(serialize = "\x69")]
    AsciiLowerI = 105,
    /// ASCII character 0x6a.
    #[strum(serialize = "\x6a")]
    AsciiLowerJ = 106,
    /// ASCII character 0x6b.
    #[strum(serialize = "\x6b")]
    AsciiLowerK = 107,
    /// ASCII character 0x6c.
    #[strum(serialize = "\x6c")]
    AsciiLowerL = 108,
    /// ASCII character 0x6d.
    #[strum(serialize = "\x6d")]
    AsciiLowerM = 109,
    /// ASCII character 0x6e.
    #[strum(serialize = "\x6e")]
    AsciiLowerN = 110,
    /// ASCII character 0x6f.
    #[strum(serialize = "\x6f")]
    AsciiLowerO = 111,
    /// ASCII character 0x70.
    #[strum(serialize = "\x70")]
    AsciiLowerP = 112,
    /// ASCII character 0x71.
    #[strum(serialize = "\x71")]
    AsciiLowerQ = 113,
    /// ASCII character 0x72.
    #[strum(serialize = "\x72")]
    AsciiLowerR = 114,
    /// ASCII character 0x73.
    #[strum(serialize = "\x73")]
    AsciiLowerS = 115,
    /// ASCII character 0x74.
    #[strum(serialize = "\x74")]
    AsciiLowerT = 116,
    /// ASCII character 0x75.
    #[strum(serialize = "\x75")]
    AsciiLowerU = 117,
    /// ASCII character 0x76.
    #[strum(serialize = "\x76")]
    AsciiLowerV = 118,
    /// ASCII character 0x77.
    #[strum(serialize = "\x77")]
    AsciiLowerW = 119,
    /// ASCII character 0x78.
    #[strum(serialize = "\x78")]
    AsciiLowerX = 120,
    /// ASCII character 0x79.
    #[strum(serialize = "\x79")]
    AsciiLowerY = 121,
    /// ASCII character 0x7a.
    #[strum(serialize = "\x7a")]
    AsciiLowerZ = 122,
    /// ASCII character 0x7b.
    #[strum(serialize = "\x7b")]
    AsciiLeftBrace = 123,
    /// ASCII character 0x7c.
    #[strum(serialize = "\x7c")]
    AsciiPipe = 124,
    /// ASCII character 0x7d.
    #[strum(serialize = "\x7d")]
    AsciiRightBrace = 125,
    /// ASCII character 0x7e.
    #[strum(serialize = "\x7e")]
    AsciiTilde = 126,
    /// ASCII character 0x7f.
    #[strum(serialize = "\x7f")]
    AsciiDelete = 127,
    /// The empty string, addressed by [`StringId::EMPTY`] immediately after ASCII.
    #[strum(serialize = "")]
    EmptyString = 128,
    /// `binascii.a2b_base64()` function.
    #[strum(serialize = "a2b_base64")]
    A2bBase64,
    /// `binascii.a2b_hex()` function, an alias of `unhexlify`.
    #[strum(serialize = "a2b_hex")]
    A2bHex,
    /// `binascii.a2b_qp()` function.
    #[strum(serialize = "a2b_qp")]
    A2bQp,
    /// `binascii.a2b_uu()` function.
    #[strum(serialize = "a2b_uu")]
    A2bUu,
    /// `base64.a85decode()` function.
    #[strum(serialize = "a85decode")]
    A85Decode,
    /// `base64.a85encode()` function.
    #[strum(serialize = "a85encode")]
    A85Encode,
    /// `sys.abiflags` attribute.
    Abiflags,
    /// Kwarg name `abs_tol` — `math.isclose(abs_tol=...)`.
    AbsTol,
    /// `Path.absolute()` method — yields a host call.
    Absolute,
    /// `itertools.accumulate()` function.
    Accumulate,
    /// `math.acos()` function.
    Acos,
    /// `math.acosh()` function.
    Acosh,
    /// `set.add()` method.
    Add,
    /// `adobe` parameter of `base64.a85encode()` / `a85decode()`.
    #[strum(serialize = "adobe")]
    Adobe,
    /// `json.dumps(allow_nan=...)` keyword.
    #[strum(serialize = "allow_nan")]
    AllowNan,
    /// `alpha` parameter of `random.gammavariate()` and the other shape variates.
    Alpha,
    /// `altchars` parameter of `base64.b64encode()` / `b64decode()`.
    #[strum(serialize = "altchars")]
    Altchars,
    /// `os.altsep` constant name.
    Altsep,
    /// `typing.Annotated` marker.
    #[strum(serialize = "Annotated")]
    Annotated,
    /// `typing.Any` marker.
    #[strum(serialize = "Any")]
    Any,
    /// `sys.api_version` attribute.
    ApiVersion,
    /// `list.append()` method.
    Append,
    /// `Path.append_bytes()` method — yields a host call.
    AppendBytes,
    /// `Path.append_text()` method — yields a host call.
    AppendText,
    /// `deque.appendleft()` method.
    Appendleft,
    /// `BaseException.args` attribute.
    Args,
    /// `sys.argv` attribute.
    Argv,
    /// `Path.as_posix()` method, answered without host I/O.
    AsPosix,
    /// `re.ASCII` flag
    #[strum(serialize = "ASCII")]
    AsciiFlag,
    /// `math.asin()` function.
    Asin,
    /// `math.asinh()` function.
    Asinh,
    /// Module name for `import asyncio`.
    Asyncio,
    /// `math.atan()` function.
    Atan,
    /// `math.atan2()` function.
    Atan2,
    /// `math.atanh()` function.
    Atanh,
    /// `base64.b16decode()` function.
    #[strum(serialize = "b16decode")]
    B16Decode,
    /// `base64.b16encode()` function.
    #[strum(serialize = "b16encode")]
    B16Encode,
    /// `binascii.b2a_base64()` function.
    #[strum(serialize = "b2a_base64")]
    B2aBase64,
    /// `binascii.b2a_hex()` function, an alias of `hexlify`.
    #[strum(serialize = "b2a_hex")]
    B2aHex,
    /// `binascii.b2a_qp()` function.
    #[strum(serialize = "b2a_qp")]
    B2aQp,
    /// `binascii.b2a_uu()` function.
    #[strum(serialize = "b2a_uu")]
    B2aUu,
    /// `base64.b32decode()` function.
    #[strum(serialize = "b32decode")]
    B32Decode,
    /// `base64.b32encode()` function.
    #[strum(serialize = "b32encode")]
    B32Encode,
    /// `base64.b32hexdecode()` function.
    #[strum(serialize = "b32hexdecode")]
    B32HexDecode,
    /// `base64.b32hexencode()` function.
    #[strum(serialize = "b32hexencode")]
    B32HexEncode,
    /// `base64.b64decode()` function.
    #[strum(serialize = "b64decode")]
    B64Decode,
    /// `base64.b64encode()` function.
    #[strum(serialize = "b64encode")]
    B64Encode,
    /// `base64.b85decode()` function.
    #[strum(serialize = "b85decode")]
    B85Decode,
    /// `base64.b85encode()` function.
    #[strum(serialize = "b85encode")]
    B85Encode,
    /// `backtick` parameter of `binascii.b2a_uu()`.
    #[strum(serialize = "backtick")]
    Backtick,
    /// Kwarg name `base` — `pow(base=...)`.
    Base,
    /// Module name for `import base64`.
    #[strum(serialize = "base64")]
    Base64,
    /// `sys.base_exec_prefix` attribute.
    BaseExecPrefix,
    /// `sys.base_prefix` attribute.
    BasePrefix,
    /// `itertools.batched()` function.
    Batched,
    /// `beta` parameter of `random.gammavariate()` and the other shape variates.
    Beta,
    /// `random.betavariate()` function.
    Betavariate,
    /// Module name for `import binascii`.
    #[strum(serialize = "binascii")]
    Binascii,
    /// `random.binomialvariate()` function.
    Binomialvariate,
    /// Kwarg name `buffering` — `open(buffering=...)`.
    Buffering,
    /// `sys.builtin_module_names` attribute.
    BuiltinModuleNames,
    /// `sys.byteorder` attribute.
    Byteorder,
    /// `bytes_per_sep` parameter of `binascii.hexlify()`.
    #[strum(serialize = "bytes_per_sep")]
    BytesPerSep,
    /// `sys.flags.bytes_warning` field.
    BytesWarning,
    /// `typing.Callable` marker.
    #[strum(serialize = "Callable")]
    Callable,
    /// `capitalize()` method, shared by `str` and `bytes`.
    Capitalize,
    /// `str.casefold()` method.
    Casefold,
    /// `unicodedata.category()` function.
    Category,
    /// `math.cbrt()` function.
    Cbrt,
    /// `math.ceil()` function.
    Ceil,
    /// `center()` method, shared by `str` and `bytes`.
    Center,
    /// `itertools.chain()` function.
    Chain,
    /// `os.chdir()` function.
    Chdir,
    /// `random.choice()` function.
    Choice,
    /// `random.choices()` function.
    Choices,
    /// `__class_getitem__`, the classmethod behind `list[int]`.
    #[strum(serialize = "__class_getitem__")]
    ClassGetitem,
    /// `typing.ClassVar` marker.
    #[strum(serialize = "ClassVar")]
    ClassVar,
    /// `clear()` method, shared by `list`, `dict` and `set`.
    Clear,
    /// `file.close()` method.
    Close,
    /// `file.closed` attribute.
    Closed,
    /// Kwarg name `closefd` — `open(closefd=...)`.
    Closefd,
    /// The class parameter of the decorator `@dataclass(...)` returns, which
    /// CPython spells `def wrap(cls)` and so accepts by keyword.
    Cls,
    /// `gc.collect()` function.
    Collect,
    /// Module name for `import collections`.
    Collections,
    /// `math.comb()` function.
    Comb,
    /// `itertools.combinations()` function.
    Combinations,
    /// `itertools.combinations_with_replacement()` function.
    #[strum(serialize = "combinations_with_replacement")]
    CombinationsWithReplacement,
    /// `datetime.combine()` class method.
    Combine,
    /// `unicodedata.combining()` function.
    Combining,
    /// `re.compile()` function
    Compile,
    /// `itertools.compress()` function.
    Compress,
    /// `copy()` method, shared by `list`, `dict` and `set`; also the `copy` module and `copy.copy()`.
    Copy,
    /// `sys.copyright` attribute.
    Copyright,
    /// `math.copysign()` function.
    Copysign,
    /// `math.cos()` function.
    Cos,
    /// `math.cosh()` function.
    Cosh,
    /// `count()` method, shared by `str`, `bytes`, `list` and `tuple`; also `itertools.count()`.
    Count,
    /// The `collections.Counter` type/factory.
    #[strum(serialize = "Counter")]
    Counter,
    /// `counts` parameter of `random.sample()`.
    Counts,
    /// `crc` parameter of `binascii.crc32()`.
    #[strum(serialize = "crc")]
    Crc,
    /// `binascii.crc32()` function.
    #[strum(serialize = "crc32")]
    Crc32,
    /// `binascii.crc_hqx()` function.
    #[strum(serialize = "crc_hqx")]
    CrcHqx,
    /// `cum_weights` parameter of `random.choices()`.
    CumWeights,
    /// `os.curdir` constant name.
    Curdir,
    /// `Path.cwd()` classmethod: answered from the VM's working directory, no host call.
    Cwd,
    /// `itertools.cycle()` function.
    Cycle,
    /// `data` keyword argument of `itertools.compress()`.
    Data,
    /// `dataclasses.dataclass` decorator.
    Dataclass,
    /// The `__dataclass_fields__` class attribute `@dataclass` writes: the
    /// name -> `Field` mapping that drives every synthesized dunder.
    #[strum(serialize = "__dataclass_fields__")]
    DataclassFields,
    /// The `__dataclass_params__` class attribute `@dataclass` writes: the
    /// options the class was decorated with.
    #[strum(serialize = "__dataclass_params__")]
    DataclassParams,
    /// Module name for `import dataclasses`.
    Dataclasses,
    /// The `datetime.date` type.
    Date,
    /// Module name for `import datetime`, and the `datetime.datetime` type.
    Datetime,
    /// `date` / `datetime` `day` attribute and constructor kwarg.
    Day,
    /// `timedelta.days` attribute and constructor kwarg.
    Days,
    /// `sys.flags.debug` field.
    Debug,
    /// `bytes.decode()` method.
    Decode,
    /// `base64.decodebytes()` function.
    #[strum(serialize = "decodebytes")]
    Decodebytes,
    /// `copy.deepcopy()`. The module name and `copy.copy()` reuse [`Self::Copy`].
    Deepcopy,
    /// Kwarg name `default` — `os.getenv(default=...)`.
    Default,
    /// `defaultdict.default_factory` attribute.
    #[strum(serialize = "default_factory")]
    DefaultFactory,
    /// The `collections.defaultdict` factory function.
    Defaultdict,
    /// `namedtuple(..., defaults=...)` keyword argument.
    Defaults,
    /// `math.degrees()` function.
    Degrees,
    /// `delay` parameter of `asyncio.sleep()`.
    Delay,
    /// The `collections.deque` type.
    Deque,
    /// `sys.flags.dev_mode` field.
    DevMode,
    /// Value of `os.devnull`.
    #[strum(serialize = "/dev/null")]
    DevNullString,
    /// `os.devnull` constant name.
    Devnull,
    /// `typing.Dict` marker.
    #[strum(serialize = "Dict")]
    DictType,
    /// `set.difference()` method.
    Difference,
    /// `sys.float_info.dig` field.
    Dig,
    /// Kwarg name `dir_fd` — `os.stat(dir_fd=...)`, `os.mkdir(dir_fd=...)`, etc.
    DirFd,
    /// `gc.disable()` function.
    Disable,
    /// `set.discard()` method.
    Discard,
    /// `math.dist()` function.
    Dist,
    /// `sys.dont_write_bytecode` attribute.
    DontWriteBytecode,
    /// `re.DOTALL` flag
    #[strum(serialize = "DOTALL")]
    DotallFlag,
    /// `itertools.dropwhile()` function.
    Dropwhile,
    /// Kwarg name `dst` — `os.rename(dst=...)`, `os.replace(dst=...)`.
    Dst,
    /// Kwarg name `dst_dir_fd` — `os.rename(dst_dir_fd=...)`.
    DstDirFd,
    /// `json.dumps()` function.
    Dumps,
    /// `__args__` of a `types.GenericAlias`.
    #[strum(serialize = "__args__")]
    DunderArgs,
    /// `__doc__` — synthesized into the namespace of classes created by the
    /// 3-arg `type()` builtin when the caller's dict omits it.
    #[strum(serialize = "__doc__")]
    DunderDoc,
    /// `__getnewargs__` — the copy/pickle hook on named tuples.
    #[strum(serialize = "__getnewargs__")]
    DunderGetnewargs,
    /// `__main__`, the `__name__` of the module being run.
    #[strum(serialize = "__main__")]
    DunderMain,
    /// `defaultdict.__missing__` method.
    #[strum(serialize = "__missing__")]
    DunderMissing,
    /// `__module__` — the defining module name, exposed on namedtuple classes.
    #[strum(serialize = "__module__")]
    DunderModule,
    /// `__name__` attribute of a type.
    #[strum(serialize = "__name__")]
    DunderName,
    /// `__origin__` of a `types.GenericAlias`.
    #[strum(serialize = "__origin__")]
    DunderOrigin,
    /// `__parameters__` of a `types.GenericAlias`.
    #[strum(serialize = "__parameters__")]
    DunderParameters,
    /// `__qualname__` — the qualified class name, exposed on namedtuple classes.
    #[strum(serialize = "__qualname__")]
    DunderQualname,
    /// `Counter.elements()` method.
    Elements,
    /// `repr()`/`str()` text of `Ellipsis`, interned so rendering allocates nothing.
    #[strum(serialize = "Ellipsis")]
    EllipsisRepr,
    /// `gc.enable()` function.
    Enable,
    /// `str.encode()` method.
    Encode,
    /// `base64.encodebytes()` function.
    #[strum(serialize = "encodebytes")]
    Encodebytes,
    /// `file.encoding` attribute and the `open(encoding=...)` kwarg.
    Encoding,
    /// `match.end()` method
    End,
    /// `endswith()` method, shared by `str` and `bytes`.
    Endswith,
    /// `json.dumps(ensure_ascii=...)` keyword.
    #[strum(serialize = "ensure_ascii")]
    EnsureAscii,
    /// `__enter__`, the context-manager entry method.
    #[strum(serialize = "__enter__")]
    Enter,
    /// `os.environ` attribute.
    Environ,
    /// `sys.float_info.epsilon` field.
    Epsilon,
    /// `@dataclass(eq=...)`.
    Eq,
    /// `math.erf()` function.
    Erf,
    /// `math.erfc()` function.
    Erfc,
    /// `re.error` exception alias (same as `re.PatternError`)
    #[strum(serialize = "error")]
    Error,
    /// `binascii.Error` exception class — distinct from [`Self::Error`], which
    /// is the lowercase `re.error` alias.
    #[strum(serialize = "Error")]
    ErrorClass,
    /// Kwarg name `errors` — `open(errors=...)`.
    Errors,
    /// `re.escape()` function
    Escape,
    /// `sys.exec_prefix` attribute.
    ExecPrefix,
    /// `sys.executable` attribute.
    Executable,
    /// Kwarg name `exist_ok` — `Path.mkdir(exist_ok=...)`.
    ExistOk,
    /// `Path.exists()` method — yields a host call.
    Exists,
    /// `__exit__`, the context-manager exit method.
    #[strum(serialize = "__exit__")]
    Exit,
    /// `math.exp()` function.
    Exp,
    /// `math.exp2()` function.
    Exp2,
    /// `str.expandtabs()` method.
    Expandtabs,
    /// `math.expm1()` function.
    Expm1,
    /// `random.expovariate()` function.
    Expovariate,
    /// `extend()` method, shared by `list` and `deque`.
    Extend,
    /// `deque.extendleft()` method.
    Extendleft,
    /// `os.extsep` constant name.
    Extsep,
    /// `math.fabs()` function.
    Fabs,
    /// `math.factorial()` function.
    Factorial,
    /// `repr()`/`str()` text of `False`, interned so rendering allocates nothing.
    #[strum(serialize = "False")]
    FalseRepr,
    /// `namedtuple(field_names=...)` keyword argument.
    #[strum(serialize = "field_names")]
    FieldNames,
    /// Kwarg name `file` — `open(file=...)`.
    File,
    /// `zip_longest(fillvalue=...)` keyword.
    Fillvalue,
    /// `itertools.filterfalse()` function.
    Filterfalse,
    /// Value of `sys.version_info.releaselevel`.
    Final,
    /// `typing.Final` marker.
    #[strum(serialize = "Final")]
    FinalType,
    /// `find()` method, shared by `str` and `bytes`.
    Find,
    /// `re.findall()` / `pattern.findall()` method
    Findall,
    /// `re.finditer()` / `pattern.finditer()` method
    Finditer,
    /// `pattern.flags`
    Flags,
    /// `sys.float_info` attribute.
    FloatInfo,
    /// `sys.float_repr_style` attribute.
    FloatReprStyle,
    /// `math.floor()` function.
    Floor,
    /// `file.flush()` method.
    Flush,
    /// `math.fma()` function.
    Fma,
    /// `math.fmod()` function.
    Fmod,
    /// `datetime` / `time` `fold` attribute and constructor kwarg.
    Fold,
    /// `foldspaces` parameter of `base64.a85encode()` / `a85decode()`.
    #[strum(serialize = "foldspaces")]
    Foldspaces,
    /// Kwarg name `follow_symlinks` — `os.stat(follow_symlinks=...)`.
    FollowSymlinks,
    /// Kwarg name `format` — `date.strftime(format=...)`, `datetime.strftime(format=...)`.
    Format,
    /// `math.frexp()` function.
    Frexp,
    /// `chain.from_iterable` — the one attribute an `itertools` type carries.
    #[strum(serialize = "from_iterable")]
    FromIterable,
    /// `bytes.fromhex()` classmethod.
    Fromhex,
    /// `date.fromisoformat()` / `datetime.fromisoformat()` classmethod.
    Fromisoformat,
    /// `dict.fromkeys()` classmethod.
    Fromkeys,
    /// `@dataclass(frozen=...)`.
    Frozen,
    /// `dataclasses.FrozenInstanceError` exception.
    #[strum(serialize = "FrozenInstanceError")]
    FrozenInstanceError,
    /// `typing.FrozenSet` marker.
    #[strum(serialize = "FrozenSet")]
    FrozenSet,
    /// `Path.__fspath__()` method, answered without host I/O.
    #[strum(serialize = "__fspath__")]
    Fspath,
    /// `math.fsum()` function.
    Fsum,
    /// `re.fullmatch()` / `pattern.fullmatch()` method
    Fullmatch,
    /// `partial.func` attribute, and the `accumulate(func=...)` keyword.
    Func,
    /// Module name for `import functools`.
    Functools,
    /// `math.gamma()` function.
    Gamma,
    /// `random.gammavariate()` function.
    Gammavariate,
    /// `asyncio.gather()` function.
    Gather,
    /// `random.gauss()` function.
    Gauss,
    /// Module name for `import gc`.
    Gc,
    /// `math.gcd()` function.
    Gcd,
    /// `typing.Generator` marker.
    #[strum(serialize = "Generator")]
    Generator,
    /// `typing.Generic` marker.
    #[strum(serialize = "Generic")]
    Generic,
    /// `dict.get()` method.
    Get,
    /// `os.getcwd()` function.
    Getcwd,
    /// `os.getcwdb()` function.
    Getcwdb,
    /// `os.getenv()` function.
    Getenv,
    /// `random.getrandbits()` function.
    Getrandbits,
    /// `random.getstate()` function.
    Getstate,
    /// `match.group()` method
    Group,
    /// `itertools.groupby()` function.
    Groupby,
    /// `match.groupdict()` method
    Groupdict,
    /// `itertools._grouper`, the sub-iterator `groupby` hands out.
    #[strum(serialize = "_grouper")]
    Grouper,
    /// `match.groups()` method
    Groups,
    /// `sys.flags.hash_randomization` field.
    HashRandomization,
    /// `header` parameter of the `binascii` quoted-printable pair.
    #[strum(serialize = "header")]
    Header,
    /// `bytes.hex()` method.
    Hex,
    /// `binascii.hexlify()` function.
    #[strum(serialize = "hexlify")]
    Hexlify,
    /// `hexstr` parameter of `binascii.unhexlify()`.
    #[strum(serialize = "hexstr")]
    Hexstr,
    /// `sys.hexversion` attribute.
    Hexversion,
    /// `high` parameter of `random.triangular()`.
    High,
    /// `datetime` / `time` `hour` attribute and constructor kwarg.
    Hour,
    /// `timedelta(hours=...)` constructor kwarg.
    Hours,
    /// `math.hypot()` function.
    Hypot,
    /// `sys.flags.ignore_environment` field.
    IgnoreEnvironment,
    /// `re.IGNORECASE` flag
    #[strum(serialize = "IGNORECASE")]
    Ignorecase,
    /// `ignorechars` parameter of `base64.a85decode()`.
    #[strum(serialize = "ignorechars")]
    Ignorechars,
    /// `binascii.Incomplete` exception class.
    #[strum(serialize = "Incomplete")]
    IncompleteClass,
    /// `json.dumps(indent=...)` keyword.
    Indent,
    /// `index()` method, shared by `str`, `bytes`, `list`, `tuple` and `deque`.
    Index,
    /// `@dataclass(init=...)`.
    Init,
    /// `initial` keyword argument of `functools.reduce()` and
    /// `itertools.accumulate()`.
    Initial,
    /// `list.insert()` method.
    Insert,
    /// `sys.flags.inspect` field.
    Inspect,
    /// `sys.flags.int_max_str_digits` field.
    IntMaxStrDigits,
    /// `sys.flags.interactive` field.
    Interactive,
    /// `set.intersection()` method.
    Intersection,
    /// `Path.is_absolute()` method, answered without host I/O.
    IsAbsolute,
    /// `dataclasses.is_dataclass()` function.
    IsDataclass,
    /// `Path.is_dir()` method — yields a host call.
    IsDir,
    /// `Path.is_file()` method — yields a host call.
    IsFile,
    /// `unicodedata.is_normalized()` function.
    #[strum(serialize = "is_normalized")]
    IsNormalized,
    /// `Path.is_symlink()` method — yields a host call.
    IsSymlink,
    /// `isalnum()` method, shared by `str` and `bytes`.
    Isalnum,
    /// `isalpha()` method, shared by `str` and `bytes`.
    Isalpha,
    /// `isascii()` method, shared by `str` and `bytes`.
    Isascii,
    /// `math.isclose()` function.
    Isclose,
    /// `str.isdecimal()` method.
    Isdecimal,
    /// `isdigit()` method, shared by `str` and `bytes`.
    Isdigit,
    /// `set.isdisjoint()` method.
    Isdisjoint,
    /// `math.isfinite()` function.
    Isfinite,
    /// `str.isidentifier()` method.
    Isidentifier,
    /// `math.isinf()` function.
    Isinf,
    /// `itertools.islice()` function.
    Islice,
    /// `islower()` method, shared by `str` and `bytes`.
    Islower,
    /// `math.isnan()` function.
    Isnan,
    /// `str.isnumeric()` method.
    Isnumeric,
    /// `date.isoformat()` / `datetime.isoformat()` method.
    Isoformat,
    /// `sys.flags.isolated` field.
    Isolated,
    /// `date.isoweekday()` / `datetime.isoweekday()` method.
    Isoweekday,
    /// `str.isprintable()` method.
    Isprintable,
    /// `math.isqrt()` function.
    Isqrt,
    /// `isspace()` method, shared by `str` and `bytes`.
    Isspace,
    /// `set.issubset()` method.
    Issubset,
    /// `set.issuperset()` method.
    Issuperset,
    /// `istext` parameter of `binascii.b2a_qp()`.
    #[strum(serialize = "istext")]
    Istext,
    /// `istitle()` method, shared by `str` and `bytes`.
    Istitle,
    /// `isupper()` method, shared by `str` and `bytes`.
    Isupper,
    /// `dict.items()` method.
    Items,
    /// `typing.Iterable` marker.
    #[strum(serialize = "Iterable")]
    Iterable,
    /// `deque(iterable=...)` — the constructor's first parameter, which CPython
    /// also accepts by keyword. Distinct from [`Self::Iterable`], which is the
    /// capitalized `typing.Iterable`.
    #[strum(serialize = "iterable")]
    IterableArg,
    /// `typing.Iterator` marker.
    #[strum(serialize = "Iterator")]
    IteratorType,
    /// `Path.iterdir()` method — yields a host call.
    Iterdir,
    /// Module name for `import itertools`.
    Itertools,
    /// `join()` method, shared by `str` and `bytes`.
    Join,
    /// `Path.joinpath()` method, answered without host I/O.
    Joinpath,
    /// Module name for `import json`.
    Json,
    /// `json.JSONDecodeError` exception.
    #[strum(serialize = "JSONDecodeError")]
    JsonDecodeError,
    /// `kappa` parameter of `random.vonmisesvariate()`.
    Kappa,
    /// Kwarg name `keepends` — `str.splitlines(keepends=...)`.
    Keepends,
    /// Kwarg name `key` — `sorted(key=...)`, `min(key=...)`, etc.
    Key,
    /// `dict.keys()` method.
    Keys,
    /// `partial.keywords` attribute.
    Keywords,
    /// `@dataclass(kw_only=...)`.
    KwOnly,
    /// `lambd` parameter of `random.expovariate()`.
    Lambd,
    /// `math.lcm()` function.
    Lcm,
    /// `math.ldexp()` function.
    Ldexp,
    /// `math.lgamma()` function.
    Lgamma,
    /// The value of `sys.platlibdir`.
    Lib,
    /// `os.linesep` constant name.
    Linesep,
    /// `typing.List` marker.
    #[strum(serialize = "List")]
    ListType,
    /// `os.listdir()` function.
    Listdir,
    /// `typing.Literal` marker.
    #[strum(serialize = "Literal")]
    Literal,
    /// The value of `sys.byteorder` on every target Monty builds for.
    Little,
    /// `ljust()` method, shared by `str` and `bytes`.
    Ljust,
    /// `json.loads()` function.
    Loads,
    /// `math.log()` function.
    Log,
    /// `math.log10()` function.
    Log10,
    /// `math.log1p()` function.
    Log1p,
    /// `math.log2()` function.
    Log2,
    /// `random.lognormvariate()` function.
    Lognormvariate,
    /// `unicodedata.lookup()` function.
    Lookup,
    /// `low` parameter of `random.triangular()`.
    Low,
    /// `lower()` method, shared by `str` and `bytes`.
    Lower,
    /// `lstrip()` method, shared by `str` and `bytes`.
    Lstrip,
    /// `sys.version_info.major` field.
    Major,
    /// `os.makedirs()` function.
    Makedirs,
    /// `sys.float_info.mant_dig` field.
    MantDig,
    /// `map01` parameter of `base64.b32decode()`.
    #[strum(serialize = "map01")]
    Map01,
    /// `typing.Mapping` marker.
    #[strum(serialize = "Mapping")]
    Mapping,
    /// `re.match()` / `pattern.match()` method
    Match,
    /// `@dataclass(match_args=...)`.
    MatchArgs,
    /// `re.Match`
    #[strum(serialize = "Match")]
    MatchClass,
    /// Module name for `import math`.
    Math,
    /// `math.inf` constant
    #[strum(serialize = "inf")]
    MathInf,
    /// `math.nan` constant
    #[strum(serialize = "nan")]
    MathNan,
    /// `sys.float_info.max` field; also the `max` class constant of the `datetime` classes.
    Max,
    /// `sys.float_info.max_10_exp` field.
    #[strum(serialize = "max_10_exp")]
    Max10Exp,
    /// `base64.MAXBINSIZE` module constant.
    #[strum(serialize = "MAXBINSIZE")]
    MaxBinSize,
    /// `sys.float_info.max_exp` field.
    MaxExp,
    /// `base64.MAXLINESIZE` module constant.
    #[strum(serialize = "MAXLINESIZE")]
    MaxLineSize,
    /// `deque.maxlen` attribute (also a constructor keyword argument).
    Maxlen,
    /// `sys.maxsize` attribute.
    Maxsize,
    /// Kwarg name `maxsplit` — `str.split(maxsplit=...)`, `re.split(maxsplit=...)`.
    Maxsplit,
    /// `sys.maxunicode` attribute.
    Maxunicode,
    /// `memo` parameter of `copy.deepcopy()`.
    Memo,
    /// `sys.version_info.micro` field.
    Micro,
    /// `datetime` / `time` `microsecond` attribute and constructor kwarg.
    Microsecond,
    /// `timedelta.microseconds` attribute and constructor kwarg.
    Microseconds,
    /// `timedelta(milliseconds=...)` constructor kwarg.
    Milliseconds,
    /// `sys.float_info.min` field; also the `min` class constant of the `datetime` classes.
    Min,
    /// `sys.float_info.min_10_exp` field.
    #[strum(serialize = "min_10_exp")]
    Min10Exp,
    /// `sys.float_info.min_exp` field.
    MinExp,
    /// `sys.version_info.minor` field.
    Minor,
    /// `datetime` / `time` `minute` attribute and constructor kwarg.
    Minute,
    /// `timedelta(minutes=...)` constructor kwarg.
    Minutes,
    /// `Path.mkdir()` and `os.mkdir()` — yields a host call.
    Mkdir,
    /// `file.mode` attribute and the `open(mode=...)` kwarg.
    Mode,
    /// `math.modf()` function.
    Modf,
    /// `<module>`, the traceback frame name for top-level code.
    #[strum(serialize = "<module>")]
    Module,
    /// `namedtuple(..., module=...)` keyword argument.
    #[strum(serialize = "module")]
    ModuleKwarg,
    /// `date` / `datetime` `month` attribute and constructor kwarg.
    Month,
    /// Value of `sys.platform`.
    Monty,
    /// The value of `sys.copyright`.
    #[strum(serialize = "Copyright (c) Pydantic Services Inc. 2026 to present")]
    MontyCopyright,
    /// Value of `sys.version`.
    #[strum(serialize = "3.14.0 (Monty)")]
    MontyVersionString,
    /// `Counter.most_common()` method.
    #[strum(serialize = "most_common")]
    MostCommon,
    /// `mu` parameter of `random.gauss()` and the other normal variates.
    Mu,
    /// `re.MULTILINE` flag
    #[strum(serialize = "MULTILINE")]
    MultilineFlag,
    /// `Path.name` property; also `file.name`, `os.name` and `unicodedata.name()`.
    Name,
    /// The `collections.namedtuple` factory function.
    Namedtuple,
    /// Kwarg name `ndigits` — `round(ndigits=...)`.
    Ndigits,
    /// `typing.Never` marker.
    #[strum(serialize = "Never")]
    Never,
    /// Kwarg name `new` — `str.replace(new=...)`, `bytes.replace(new=...)`.
    New,
    /// Kwarg name `newline` — `open(newline=...)`.
    Newline,
    /// `math.nextafter()` function.
    Nextafter,
    /// `_nil` parameter of `copy.deepcopy()`, CPython's private sentinel.
    #[strum(serialize = "_nil")]
    NilSentinel,
    /// `re.NOFLAG` flag
    #[strum(serialize = "NOFLAG")]
    NoFlag,
    /// `typing.NoReturn` marker.
    #[strum(serialize = "NoReturn")]
    NoReturn,
    /// `sys.flags.no_site` field.
    NoSite,
    /// `sys.flags.no_user_site` field.
    NoUserSite,
    /// `repr()`/`str()` text of `None`, interned so rendering allocates nothing.
    #[strum(serialize = "None")]
    NoneRepr,
    /// `unicodedata.normalize()` function.
    Normalize,
    /// `random.normalvariate()` function.
    Normalvariate,
    /// Python's `NotImplemented` singleton representation.
    #[strum(serialize = "NotImplemented")]
    NotImplementedRepr,
    /// `datetime.now()` classmethod.
    Now,
    /// Kwarg name `number` — `round(number=...)`.
    Number,
    /// Kwarg name `obj` — `json.dumps(obj=...)`.
    Obj,
    /// Kwarg name `object` — `str(object=...)`, `itertools.repeat(object=...)`.
    Object,
    /// `timezone(offset=...)` constructor kwarg.
    Offset,
    /// Kwarg name `old` — `str.replace(old=...)`, `bytes.replace(old=...)`.
    Old,
    /// `Path.open()` and the `open()` builtin, which share the `OsFunctionCall::Open`
    /// round-trip. `Path::py_call_attr` delegates to `builtin_open` for mode/kwarg
    /// validation rather than taking the generic `is_path_os_method` pre-flight.
    Open,
    /// Kwarg name `opener` — `open(opener=...)`.
    Opener,
    /// `sys.flags.optimize` field.
    Optimize,
    /// `typing.Optional` marker.
    #[strum(serialize = "Optional")]
    Optional,
    /// `@dataclass(order=...)`.
    Order,
    /// Module name for `import os`.
    Os,
    /// `os.fspath()` function — distinct from `Fspath` (`__fspath__`).
    #[strum(serialize = "fspath")]
    OsFspath,
    /// Named-tuple type name of the `os.stat()` result.
    #[strum(serialize = "StatResult")]
    OsStatResult,
    /// `pad` parameter of `base64.b85encode()`.
    #[strum(serialize = "pad")]
    Pad,
    /// `itertools.pairwise()` function.
    Pairwise,
    /// `os.pardir` constant name.
    Pardir,
    /// `Path.parent` property.
    Parent,
    /// Value of `os.pardir`.
    #[strum(serialize = "..")]
    ParentDirString,
    /// Kwarg name `parents` — `Path.mkdir(parents=...)`.
    Parents,
    /// `random.paretovariate()` function.
    Paretovariate,
    /// `functools.partial` type.
    Partial,
    /// `partition()` method, shared by `str` and `bytes`.
    Partition,
    /// `Path.parts` property.
    Parts,
    /// Kwarg name `path` — `os.listdir(path=...)`, `os.stat(path=...)`, etc.
    Path,
    /// The `pathlib.Path` type.
    #[strum(serialize = "Path")]
    PathClass,
    /// Module name for `import pathlib`.
    Pathlib,
    /// `pattern.pattern`
    #[strum(serialize = "pattern")]
    PatternAttr,
    /// `re.Pattern`
    #[strum(serialize = "Pattern")]
    PatternClass,
    /// `re.PatternError` exception
    #[strum(serialize = "PatternError")]
    PatternError,
    /// `math.perm()` function.
    Perm,
    /// `itertools.permutations()` function.
    Permutations,
    /// `math.pi` constant
    Pi,
    /// `sys.platform` attribute.
    Platform,
    /// `sys.platlibdir` attribute.
    Platlibdir,
    /// `pop()` method, shared by `list`, `dict`, `set` and `deque`.
    Pop,
    /// `dict.popitem()` method.
    Popitem,
    /// `deque.popleft()` method.
    Popleft,
    /// `population` parameter of `random.choices()` and `random.sample()`.
    Population,
    /// Value of `os.name`.
    Posix,
    /// `math.pow()` function.
    Pow,
    /// `sys.prefix` attribute.
    Prefix,
    /// `math.prod()` function.
    Prod,
    /// `itertools.product()` function.
    Product,
    /// `typing.Protocol` marker.
    #[strum(serialize = "Protocol")]
    Protocol,
    /// `sys.pycache_prefix` attribute.
    PycachePrefix,
    /// `sys.flags.quiet` field.
    Quiet,
    /// `quotetabs` parameter of `binascii.b2a_qp()`.
    #[strum(serialize = "quotetabs")]
    Quotetabs,
    /// `math.radians()` function.
    Radians,
    /// `sys.float_info.radix` field.
    Radix,
    /// `random.randbytes()` function.
    Randbytes,
    /// `random.randint()` function.
    Randint,
    /// Module name for `import random`, and the `random.random()` function.
    Random,
    /// The `random.Random` class.
    #[strum(serialize = "Random")]
    RandomClass,
    /// `Random.VERSION`, the `getstate()` format number.
    #[strum(serialize = "VERSION")]
    RandomVersion,
    /// `random.randrange()` function.
    Randrange,
    /// Module name for `import re`.
    Re,
    /// `file.read()` method.
    Read,
    /// `Path.read_bytes()` method — yields a host call.
    ReadBytes,
    /// `Path.read_text()` method — yields a host call.
    ReadText,
    /// `file.readable()` method.
    Readable,
    /// `file.readline()` method.
    Readline,
    /// `file.readlines()` method.
    Readlines,
    /// `functools.reduce()` function.
    Reduce,
    /// Kwarg name `rel_tol` — `math.isclose(rel_tol=...)`.
    RelTol,
    /// `sys.version_info.releaselevel` field.
    Releaselevel,
    /// `math.remainder()` function.
    Remainder,
    /// `remove()` method, shared by `set`, `list` and `deque`.
    Remove,
    /// `removeprefix()` method, shared by `str` and `bytes`.
    Removeprefix,
    /// `removesuffix()` method, shared by `str` and `bytes`.
    Removesuffix,
    /// `Path.rename()` and `os.rename()` — yields a host call.
    Rename,
    /// `itertools.repeat()` function.
    Repeat,
    /// Kwarg name `repl` — `re.sub(repl=...)`.
    Repl,
    /// `replace()` method, shared by `str` and `bytes`.
    Replace,
    /// `@dataclass(repr=...)`.
    Repr,
    /// `resolution` class constant of the `datetime` classes.
    Resolution,
    /// `Path.resolve()` method — yields a host call.
    Resolve,
    /// `result` parameter of `asyncio.sleep()`.
    #[strum(serialize = "result")]
    ResultArg,
    /// Kwarg name `return_exceptions` — `asyncio.gather(return_exceptions=...)`.
    ReturnExceptions,
    /// `list.reverse()` method.
    Reverse,
    /// `rfind()` method, shared by `str` and `bytes`.
    Rfind,
    /// `rindex()` method, shared by `str` and `bytes`.
    Rindex,
    /// `rjust()` method, shared by `str` and `bytes`.
    Rjust,
    /// `Path.rmdir()` and `os.rmdir()` — yields a host call.
    Rmdir,
    /// `deque.rotate()` method.
    Rotate,
    /// `sys.float_info.rounds` field.
    Rounds,
    /// `rpartition()` method, shared by `str` and `bytes`.
    Rpartition,
    /// `rsplit()` method, shared by `str` and `bytes`.
    Rsplit,
    /// `rstrip()` method, shared by `str` and `bytes`.
    Rstrip,
    /// `asyncio.run()` function.
    Run,
    /// `sys.flags.safe_path` field.
    SafePath,
    /// `random.sample()` function.
    Sample,
    /// `re.search()` / `pattern.search()` method
    Search,
    /// `datetime` / `time` `second` attribute and constructor kwarg.
    Second,
    /// `timedelta.seconds` attribute and constructor kwarg.
    Seconds,
    /// `random.seed()` function.
    Seed,
    /// `file.seek()` method.
    Seek,
    /// `file.seekable()` method.
    Seekable,
    /// `selectors` keyword argument of `itertools.compress()`.
    Selectors,
    /// `typing.Self` marker.
    #[strum(serialize = "Self")]
    SelfType,
    /// Kwarg name `sep` — `str.split(sep=...)`, `print(sep=...)`, etc.
    Sep,
    /// `json.dumps(separators=...)` keyword.
    Separators,
    /// `seq` parameter of `random.choice()`.
    Seq,
    /// `typing.Sequence` marker.
    #[strum(serialize = "Sequence")]
    Sequence,
    /// `sys.version_info.serial` field.
    Serial,
    /// `typing.Set` marker.
    #[strum(serialize = "Set")]
    SetType,
    /// `dict.setdefault()` method.
    Setdefault,
    /// `sys.setrecursionlimit()` function (only callable under `test-hooks`).
    Setrecursionlimit,
    /// `random.setstate()` function.
    Setstate,
    /// The value of `sys.float_repr_style`.
    Short,
    /// `random.shuffle()` function.
    Shuffle,
    /// `sigma` parameter of `random.gauss()` and the other normal variates.
    Sigma,
    /// `math.sin()` function.
    Sin,
    /// `math.sinh()` function.
    Sinh,
    /// `size` parameter of `os.urandom()`.
    Size,
    /// `json.dumps(skipkeys=...)` keyword.
    Skipkeys,
    /// `time.sleep()` and `asyncio.sleep()`.
    Sleep,
    /// `@dataclass(slots=...)`.
    Slots,
    /// `list.sort()` method.
    Sort,
    /// `json.dumps(sort_keys=...)` keyword.
    #[strum(serialize = "sort_keys")]
    SortKeys,
    /// Kwarg name `source` — `bytes(source=...)`, `bytearray(source=...)`.
    Source,
    /// `match.span()` method
    Span,
    /// `split()` method, shared by `str` and `bytes`; also `re.split()`.
    Split,
    /// `splitlines()` method, shared by `str` and `bytes`.
    Splitlines,
    /// `math.sqrt()` function.
    Sqrt,
    /// Kwarg name `src` — `os.rename(src=...)`, `os.replace(src=...)`.
    Src,
    /// Kwarg name `src_dir_fd` — `os.rename(src_dir_fd=...)`.
    SrcDirFd,
    /// `os.stat_result.st_atime` field.
    StAtime,
    /// `os.stat_result.st_ctime` field.
    StCtime,
    /// `os.stat_result.st_dev` field.
    StDev,
    /// `os.stat_result.st_gid` field.
    StGid,
    /// `os.stat_result.st_ino` field.
    StIno,
    /// `os.stat_result.st_mode` field.
    StMode,
    /// `os.stat_result.st_mtime` field.
    StMtime,
    /// `os.stat_result.st_nlink` field.
    StNlink,
    /// `os.stat_result.st_size` field.
    StSize,
    /// `os.stat_result.st_uid` field.
    StUid,
    /// `base64.standard_b64decode()` function.
    #[strum(serialize = "standard_b64decode")]
    StandardB64Decode,
    /// `base64.standard_b64encode()` function.
    #[strum(serialize = "standard_b64encode")]
    StandardB64Encode,
    /// `itertools.starmap()` function.
    Starmap,
    /// `slice.start` attribute; also the `start` kwarg of `itertools.count()`.
    Start,
    /// `startswith()` method, shared by `str` and `bytes`.
    Startswith,
    /// `Path.stat()` and `os.stat()` — yields a host call.
    #[strum(serialize = "stat")]
    StatMethod,
    /// `state` parameter of `Random.setstate()`.
    State,
    /// `sys.stderr` attribute and the marker it holds.
    Stderr,
    /// `sys.stdout` attribute and the marker it holds.
    Stdout,
    /// `Path.stem` property.
    Stem,
    /// `slice.step` attribute; also the `step` kwarg of `itertools.count()`.
    Step,
    /// `slice.stop` attribute.
    Stop,
    /// `date.strftime()` / `datetime.strftime()` method.
    Strftime,
    /// Kwarg name `strict` — `zip(strict=...)`.
    Strict,
    /// `strict_mode` parameter of `binascii.a2b_base64()`.
    #[strum(serialize = "strict_mode")]
    StrictMode,
    /// `match.string`
    #[strum(serialize = "string")]
    StringAttr,
    /// `strip()` method, shared by `str` and `bytes`.
    Strip,
    /// `datetime.strptime()` classmethod.
    Strptime,
    /// `re.sub()` / `pattern.sub()` method
    Sub,
    /// `Counter.subtract()` method.
    Subtract,
    /// `Path.suffix` property.
    Suffix,
    /// `Path.suffixes` property.
    Suffixes,
    /// `math.sumprod()` function.
    Sumprod,
    /// `swapcase()` method, shared by `str` and `bytes`.
    Swapcase,
    /// `set.symmetric_difference()` method.
    SymmetricDifference,
    /// Module name for `import sys`.
    Sys,
    /// Named-tuple type name of `sys.flags`.
    #[strum(serialize = "sys.flags")]
    SysFlags,
    /// Named-tuple type name of `sys.float_info`.
    #[strum(serialize = "sys.float_info")]
    SysFloatInfo,
    /// Named-tuple type name of `sys.version_info`.
    #[strum(serialize = "sys.version_info")]
    SysVersionInfo,
    /// Kwarg name `tabsize` — `str.expandtabs(tabsize=...)`.
    Tabsize,
    /// `itertools.takewhile()` function.
    Takewhile,
    /// `math.tan()` function.
    Tan,
    /// `math.tanh()` function.
    Tanh,
    /// `math.tau` constant
    Tau,
    /// `itertools.tee()` function.
    Tee,
    /// `itertools._tee_dataobject`, the buffer those iterators share.
    #[strum(serialize = "_tee_dataobject")]
    TeeDataObject,
    /// `itertools._tee`, the iterator `tee()` hands out.
    #[strum(serialize = "_tee")]
    TeeType,
    /// `file.tell()` method.
    Tell,
    /// `datetime.time` class name.
    Time,
    /// The `datetime.timedelta` type.
    Timedelta,
    /// `times` keyword argument of `itertools.repeat()`.
    Times,
    /// `timespec` keyword of `time.isoformat()` and `datetime.isoformat()`.
    Timespec,
    /// `datetime.timestamp()` method.
    Timestamp,
    /// `datetime.timetz` method name.
    Timetz,
    /// The `datetime.timezone` type.
    Timezone,
    /// `title()` method, shared by `str` and `bytes`.
    Title,
    /// `date.today()` / `datetime.today()` classmethod.
    Today,
    /// `Counter.total()` method.
    Total,
    /// `timedelta.total_seconds()` method.
    TotalSeconds,
    /// `random.triangular()` function.
    Triangular,
    /// `repr()`/`str()` text of `True`, interned so rendering allocates nothing.
    #[strum(serialize = "True")]
    TrueRepr,
    /// `math.trunc()` function.
    Trunc,
    /// `typing.Tuple` marker.
    #[strum(serialize = "Tuple")]
    TupleType,
    /// `typing.Type` marker.
    #[strum(serialize = "Type")]
    Type,
    /// `typing.TYPE_CHECKING` constant.
    #[strum(serialize = "TYPE_CHECKING")]
    TypeChecking,
    /// `typing.TypeVar` marker.
    #[strum(serialize = "TypeVar")]
    TypeVar,
    /// `namedtuple(typename=...)` keyword argument.
    Typename,
    /// Module name for `import typing`.
    Typing,
    /// `datetime.now(tz=...)` kwarg.
    Tz,
    /// `datetime.tzinfo` attribute and constructor kwarg.
    Tzinfo,
    /// `tzname()` method of `time`, `datetime` and `timezone`. (`dst()` reuses
    /// the `Dst` variant already interned for the `os` kwarg of the same name.)
    Tzname,
    /// `math.ulp()` function.
    Ulp,
    /// `NamedTuple._asdict()` method.
    #[strum(serialize = "_asdict")]
    UnderAsdict,
    /// `NamedTuple._field_defaults` — dict of defaulted field names to values.
    #[strum(serialize = "_field_defaults")]
    UnderFieldDefaults,
    /// `NamedTuple._fields` — tuple of field names.
    #[strum(serialize = "_fields")]
    UnderFields,
    /// `NamedTuple._make(iterable)` classmethod.
    #[strum(serialize = "_make")]
    UnderMake,
    /// `NamedTuple._replace(**kwargs)` method.
    #[strum(serialize = "_replace")]
    UnderReplace,
    /// `binascii.unhexlify()` function.
    #[strum(serialize = "unhexlify")]
    Unhexlify,
    /// Module name for `import unicodedata`.
    Unicodedata,
    /// `unicodedata.unidata_version` constant.
    #[strum(serialize = "unidata_version")]
    UnidataVersion,
    /// `random.uniform()` function.
    Uniform,
    /// `set.union()` method.
    Union,
    /// `typing.Union` marker.
    #[strum(serialize = "Union")]
    UnionType,
    /// `Path.unlink()` and `os.unlink()` — yields a host call.
    Unlink,
    /// `@dataclass(unsafe_hash=...)`.
    UnsafeHash,
    /// `update()` method, shared by `set` and `dict`.
    Update,
    /// `upper()` method, shared by `str` and `bytes`.
    Upper,
    /// `os.urandom()` function.
    Urandom,
    /// `base64.urlsafe_b64decode()` function.
    #[strum(serialize = "urlsafe_b64decode")]
    UrlsafeB64Decode,
    /// `base64.urlsafe_b64encode()` function.
    #[strum(serialize = "urlsafe_b64encode")]
    UrlsafeB64Encode,
    /// `timezone.utc` class constant.
    Utc,
    /// `utcoffset()` method of `time`, `datetime` and `timezone`.
    Utcoffset,
    /// `sys.flags.utf8_mode` field.
    #[strum(serialize = "utf8_mode")]
    Utf8Mode,
    /// `validate` parameter of `base64.b64decode()`.
    #[strum(serialize = "validate")]
    Validate,
    /// `dict.values()` method.
    Values,
    /// `sys.flags.verbose` field.
    Verbose,
    /// `sys.version` attribute.
    Version,
    /// `sys.version_info` attribute.
    VersionInfo,
    /// `random.vonmisesvariate()` function.
    Vonmisesvariate,
    /// `sys.flags.warn_default_encoding` field.
    WarnDefaultEncoding,
    /// `@dataclass(weakref_slot=...)`.
    WeakrefSlot,
    /// `date.weekday()` / `datetime.weekday()` method.
    Weekday,
    /// `timedelta(weeks=...)` constructor kwarg.
    Weeks,
    /// `random.weibullvariate()` function.
    Weibullvariate,
    /// `weights` parameter of `random.choices()`.
    Weights,
    /// `Path.with_name()` method, answered without host I/O.
    WithName,
    /// `Path.with_stem()` method, answered without host I/O.
    WithStem,
    /// `Path.with_suffix()` method, answered without host I/O.
    WithSuffix,
    /// `wrapcol` parameter of `base64.a85encode()`.
    #[strum(serialize = "wrapcol")]
    Wrapcol,
    /// `file.writable()` method.
    Writable,
    /// `file.write()` method.
    Write,
    /// `Path.write_bytes()` method — yields a host call.
    WriteBytes,
    /// `Path.write_text()` method — yields a host call.
    WriteText,
    /// `date` / `datetime` `year` attribute and constructor kwarg.
    Year,
    /// `base64.z85decode()` function.
    #[strum(serialize = "z85decode")]
    Z85Decode,
    /// `base64.z85encode()` function.
    #[strum(serialize = "z85encode")]
    Z85Encode,
    /// `zfill()` method, shared by `str` and `bytes`.
    Zfill,
    /// `itertools.zip_longest()` function.
    ZipLongest,
}

/// One immutable interned string with directly accessible dispatch metadata.
/// Snapshots store only text; loading reconstructs the tag and cached hash.
#[derive(Debug, Clone)]
struct InternedString {
    /// Runtime classification, independent of the string's executor-local ID.
    static_tag: Option<StaticStrings>,
    /// Text and its eagerly computed Python hash.
    text: WithHash<InternedText>,
}

/// Ownership of interned text, independent of its dispatch metadata.
#[derive(Debug, Clone)]
enum InternedText {
    /// Text recognized by this build, requiring no owned allocation.
    Static(&'static str),
    /// Source or snapshot text unknown to the static registry.
    Owned(Box<str>),
}

impl AsRef<str> for InternedText {
    fn as_ref(&self) -> &str {
        match self {
            Self::Static(text) => text,
            Self::Owned(text) => text,
        }
    }
}

impl InternedString {
    /// Creates an entry for compile-time-known text.
    fn static_string(value: StaticStrings) -> Self {
        Self {
            static_tag: Some(value),
            text: WithHash::for_str(InternedText::Static(value.into())),
        }
    }

    /// Creates an entry owning text not present in the static registry.
    fn owned(value: String) -> Self {
        Self {
            static_tag: None,
            text: WithHash::for_str(InternedText::Owned(value.into_boxed_str())),
        }
    }

    /// Returns the interned text.
    fn as_str(&self) -> &str {
        self.text.value().as_ref()
    }

    /// Returns the cached Python hash.
    fn hash(&self) -> HashValue {
        self.text.hash()
    }

    /// Returns the static tag when this build recognizes the text.
    fn static_value(&self) -> Option<StaticStrings> {
        self.static_tag
    }
}

impl serde::Serialize for InternedString {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(self.as_str(), serializer)
    }
}

impl<'de> serde::Deserialize<'de> for InternedString {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Ok(match StaticStrings::from_str(&value) {
            Ok(static_string) => Self::static_string(static_string),
            Err(_) => Self::owned(value),
        })
    }
}

/// Index into the bytes interner's storage.
///
/// Separate from `StringId` to distinguish string vs bytes literals at the type level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct BytesId(u32);

impl BytesId {
    /// Returns the raw index value.
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Index into the long integer interner's storage.
///
/// Used for integer literals that exceed i64 range. The actual `BigInt` values
/// are stored in the `Interns` table and looked up by index at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct LongIntId(u32);

impl LongIntId {
    /// Returns the raw index value.
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Unique identifier for functions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct FunctionId(u32);

impl FunctionId {
    /// Creates a FunctionId from a raw index value.
    ///
    /// Used by the bytecode VM to reconstruct FunctionIds from operands stored
    /// in bytecode. The caller is responsible for ensuring the index is valid.
    #[inline]
    pub fn from_index(index: u16) -> Self {
        Self(u32::from(index))
    }

    /// Returns the raw index value.
    #[inline]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Prehashed core strings reused when constructing independent interners.
static CORE_ENTRIES: LazyLock<Vec<InternedString>> = LazyLock::new(|| {
    CORE_STATIC_STRINGS
        .iter()
        .copied()
        .map(InternedString::static_string)
        .collect()
});

/// Append-only storage: existing references remain valid across insertion.
/// No API exposes the underlying arena's removal or mutation operations.
#[derive(Debug)]
struct StringEntries(StableHeap<InternedString>);

impl StringEntries {
    /// Reserves stable slots for a new interner.
    fn with_capacity(capacity: usize) -> Self {
        Self(StableHeap::with_capacity(capacity))
    }

    /// Returns the number of assigned executor-local slots.
    fn len(&self) -> usize {
        self.0.len()
    }

    /// Appends an immutable entry without invalidating borrowed text.
    fn push(&self, entry: InternedString) {
        self.0.allocate(entry);
    }

    /// Looks up an existing slot without exposing arena IDs to callers.
    fn get(&self, index: usize) -> Option<&InternedString> {
        (index < self.len()).then(|| self.0.get(HeapId::from_index(index)))
    }

    /// Visits entries in ID order for cloning and snapshots.
    fn iter(&self) -> impl Iterator<Item = &InternedString> {
        (0..self.len()).map(|index| &self[index])
    }
}

impl Index<usize> for StringEntries {
    type Output = InternedString;
    fn index(&self, index: usize) -> &Self::Output {
        self.get(index).expect("invalid string slot")
    }
}

impl Clone for StringEntries {
    fn clone(&self) -> Self {
        let entries = Self::with_capacity(self.len());
        for entry in self.iter() {
            entries.push(entry.clone());
        }
        entries
    }
}

impl serde::Serialize for StringEntries {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}

impl<'de> serde::Deserialize<'de> for StringEntries {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let values = <Vec<InternedString> as serde::Deserialize>::deserialize(deserializer)?;
        let entries = Self::with_capacity(values.len());
        for value in values {
            entries.push(value);
        }
        Ok(entries)
    }
}

/// Interns `s` into the executor-local string table.
///
/// ASCII and empty strings remain globally addressable; other strings receive an ordinary
/// dense interner slot. Static text retains a tag in that slot rather than
/// encoding the tag in its `StringId`.
fn intern_str(
    string_map: &mut AHashMap<String, StringId>,
    static_string_ids: &RefCell<AHashMap<StaticStrings, StringId>>,
    strings: &StringEntries,
    s: &str,
) -> StringId {
    if s.is_empty() {
        StringId::EMPTY
    } else if s.len() == 1 {
        StringId::from_ascii(s.as_bytes()[0])
    } else if let Ok(value) = StaticStrings::from_str(s) {
        intern_static(static_string_ids, strings, value)
    } else {
        *string_map.entry(s.to_owned()).or_insert_with(|| {
            let id = next_string_id(strings.len());
            strings.push(InternedString::owned(s.to_owned()));
            id
        })
    }
}

/// Interns a static tag into an append-only executor-local table.
fn intern_static(
    static_string_ids: &RefCell<AHashMap<StaticStrings, StringId>>,
    strings: &StringEntries,
    value: StaticStrings,
) -> StringId {
    let text: &'static str = value.into();
    if text.is_empty() {
        StringId::EMPTY
    } else if text.len() == 1 {
        StringId::from_ascii(text.as_bytes()[0])
    } else {
        let existing = static_string_ids.borrow().get(&value).copied();
        if let Some(id) = existing {
            id
        } else {
            let id = next_string_id(strings.len());
            strings.push(InternedString::static_string(value));
            static_string_ids.borrow_mut().insert(value, id);
            id
        }
    }
}

/// Returns the next dense executor-local string ID.
fn next_string_id(strings_len: usize) -> StringId {
    let index = strings_len + INTERN_STRING_ID_OFFSET;
    StringId(index.try_into().expect("StringId overflow"))
}

/// Reverse of [`get_str`]: the `StringId` for `s`, or `None` if never interned.
fn get_string_id_by_name(
    string_map: &AHashMap<String, StringId>,
    static_string_ids: &RefCell<AHashMap<StaticStrings, StringId>>,
    s: &str,
) -> Option<StringId> {
    if s.is_empty() {
        Some(StringId::EMPTY)
    } else if s.len() == 1 {
        Some(StringId::from_ascii(s.as_bytes()[0]))
    } else if let Ok(value) = StaticStrings::from_str(s) {
        static_string_ids.borrow().get(&value).copied()
    } else {
        string_map.get(s).copied()
    }
}

/// Looks up a string by its `StringId`.
///
/// # Panics
///
/// Panics if the ID is neither reserved nor a slot in this interner.
fn get_str(strings: &StringEntries, id: StringId) -> &str {
    if let Some(text) = RESERVED_STRS.get(id.index()) {
        text
    } else {
        strings[id.index() - INTERN_STRING_ID_OFFSET].as_str()
    }
}

/// Returns the static tag stored at `id`, if any.
#[inline]
fn get_static_string(strings: &StringEntries, id: StringId) -> Option<StaticStrings> {
    if id == StringId::EMPTY {
        Some(StaticStrings::EmptyString)
    } else if id.index() < INTERN_STRING_ID_OFFSET {
        StaticStrings::from_repr(u16::try_from(id.index()).expect("ASCII ID fits u16"))
    } else {
        strings[id.index() - INTERN_STRING_ID_OFFSET].static_tag
    }
}

/// Storage for interned strings, bytes, long integers and compiled functions.
///
/// One table serves parsing, preparation, compilation and execution. Strings
/// are deduplicated; bytes and long integers are not (large literals are rare).
/// The table is single-threaded, with static strings appendable through `&self`.
///
/// # Append-only ownership in the REPL
///
/// Snippets extend the session's table in place, keeping existing IDs stable.
/// Failed compilation rolls back appended functions; interned literals remain.
/// Execution takes ownership of the table and hands it back afterwards.
///
/// # Hash tables
///
/// String entries wrap either a static tag or owned text in [`WithHash`]; bytes
/// and long integers use `WithHash` directly. Hashes are populated eagerly at
/// intern/load time, making the runtime hash methods plain index lookups.
///
/// # Reverse string lookup
///
/// [`get_string_id_by_name`](Self::get_string_id_by_name) returns the
/// `StringId` for a host-supplied `&str`. Owned text uses an in-memory reverse
/// map; static tags use a sparse reverse map. Both are rebuilt
/// deterministically after deserialization. REPL hot paths
/// such as [`MontyRepl::call_function`](crate::MontyRepl::call_function)
/// and [`MontyRepl::has_function`](crate::MontyRepl::has_function) call this
/// per host-supplied name, so the lookup must be O(1) — not the previous
/// linear scan over `strings`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "InternsWire")]
pub(crate) struct Interns {
    strings: StringEntries,
    bytes: Vec<WithHash<Vec<u8>>>,
    long_ints: Vec<WithHash<BigInt>>,
    functions: Vec<Function>,
    /// Owned-text reverse lookup for [`Self::get_string_id_by_name`].
    #[serde(skip)]
    string_id_by_name: AHashMap<String, StringId>,
    /// Static-tag reverse lookup, rebuilt from `strings` after loading.
    #[serde(skip)]
    static_string_ids: RefCell<AHashMap<StaticStrings, StringId>>,
}

impl Default for Interns {
    fn default() -> Self {
        Self::new("")
    }
}

/// Serialized form of [`Interns`]
#[derive(serde::Deserialize)]
struct InternsWire {
    strings: StringEntries,
    bytes: Vec<WithHash<Vec<u8>>>,
    long_ints: Vec<WithHash<BigInt>>,
    functions: Vec<Function>,
}

impl From<Interns> for InternsWire {
    fn from(interns: Interns) -> Self {
        Self {
            strings: interns.strings,
            bytes: interns.bytes,
            long_ints: interns.long_ints,
            functions: interns.functions,
        }
    }
}

impl TryFrom<InternsWire> for Interns {
    type Error = String;

    fn try_from(wire: InternsWire) -> Result<Self, Self::Error> {
        let (string_id_by_name, static_string_ids) = build_string_maps(&wire.strings)?;
        let interns = Self {
            strings: wire.strings,
            bytes: wire.bytes,
            long_ints: wire.long_ints,
            functions: wire.functions,
            string_id_by_name,
            static_string_ids,
        };
        Ok(interns)
    }
}

/// Reverse maps rebuilt from the serialized ordered string table.
type StringMaps = (AHashMap<String, StringId>, RefCell<AHashMap<StaticStrings, StringId>>);

/// Rebuilds both reverse maps from the canonical ordered string table.
///
/// Duplicate text is rejected because distinct IDs for equal interned strings
/// would invalidate the ID-equality fast path used by Python string equality.
fn build_string_maps(strings: &StringEntries) -> Result<StringMaps, String> {
    let mut seen = AHashMap::with_capacity(strings.len());
    let mut string_id_by_name = AHashMap::new();
    let static_string_ids = RefCell::new(AHashMap::new());
    for (index, entry) in strings.iter().enumerate() {
        let id = next_string_id(index);
        if seen.insert(entry.as_str(), id).is_some() {
            return Err(format!("duplicate interned string {:?}", entry.as_str()));
        }
        if let Some(value) = entry.static_value() {
            static_string_ids.borrow_mut().insert(value, id);
        } else {
            string_id_by_name.insert(entry.as_str().to_owned(), id);
        }
    }
    Ok((string_id_by_name, static_string_ids))
}

impl Interns {
    /// Moves this table out while leaving a cheap, intentionally unusable placeholder.
    ///
    /// Transferring the table between a REPL session and its executor avoids
    /// full interner initialization on every feed.
    pub(crate) fn take(&mut self) -> Self {
        mem::replace(self, Self::placeholder())
    }

    /// Creates the temporary value used only while an interner is moved out.
    fn placeholder() -> Self {
        Self {
            strings: StringEntries::with_capacity(0),
            bytes: Vec::new(),
            long_ints: Vec::new(),
            functions: Vec::new(),
            string_id_by_name: AHashMap::new(),
            static_string_ids: RefCell::new(AHashMap::new()),
        }
    }

    /// Creates a table containing the core strings any execution may materialize.
    /// Other static strings are interned on demand; `code` supplies a rough
    /// capacity estimate for source literals.
    pub fn new(code: &str) -> Self {
        // Rough guess: count quotes and divide by 2 (open+close per string).
        let capacity = code.bytes().filter(|&b| b == b'"' || b == b'\'').count() >> 1;
        let interns = Self {
            strings: StringEntries::with_capacity(capacity + CORE_STATIC_STRINGS.len()),
            bytes: Vec::new(),
            long_ints: Vec::new(),
            functions: Vec::new(),
            string_id_by_name: AHashMap::with_capacity(capacity),
            static_string_ids: RefCell::new(AHashMap::with_capacity(CORE_STATIC_STRINGS.len())),
        };
        for entry in CORE_ENTRIES.iter() {
            let value = entry.static_value().expect("core entries are static");
            let id = next_string_id(interns.strings.len());
            interns.strings.push(entry.clone());
            interns.static_string_ids.borrow_mut().insert(value, id);
        }
        interns
    }

    /// Interns bytes without deduplication, since bytes literals are rare.
    pub fn intern_bytes(&mut self, b: &[u8]) -> BytesId {
        let id = BytesId(self.bytes.len().try_into().expect("BytesId overflow"));
        self.bytes.push(WithHash::for_bytes(b.to_vec()));
        id
    }

    /// Interns a big integer without deduplication, since literals exceeding i64 are rare.
    pub fn intern_long_int(&mut self, bi: BigInt) -> LongIntId {
        let id = LongIntId(self.long_ints.len().try_into().expect("LongIntId overflow"));
        self.long_ints.push(WithHash::for_long_int(bi));
        id
    }

    /// Appends a compiled function, returning its index for bytecode operands.
    pub(crate) fn push_function(&mut self, function: Function) -> usize {
        let index = self.functions.len();
        self.functions.push(function);
        index
    }

    /// Records the function count before compilation so failures can roll back.
    pub(crate) fn functions_len(&self) -> usize {
        self.functions.len()
    }

    /// Removes functions appended by a rejected compilation.
    pub(crate) fn truncate_functions(&mut self, len: usize) {
        self.functions.truncate(len);
    }

    /// Interns source or host-supplied text, deduplicating it against existing entries.
    /// ASCII and empty strings use reserved IDs; others receive stable session-local IDs.
    pub(crate) fn intern(&mut self, s: &str) -> StringId {
        intern_str(&mut self.string_id_by_name, &self.static_string_ids, &self.strings, s)
    }

    /// Interns compile-time-known text directly into the append-only table.
    pub(crate) fn intern_static(&self, value: StaticStrings) -> StringId {
        intern_static(&self.static_string_ids, &self.strings, value)
    }

    /// Looks up a string by its `StringId`.
    ///
    /// # Panics
    ///
    /// Panics if the `StringId` is invalid.
    #[inline]
    pub fn get_str(&self, id: StringId) -> &str {
        get_str(&self.strings, id)
    }

    /// Returns the static tag stored in an executor-local string slot.
    pub(crate) fn static_string(&self, id: StringId) -> Option<StaticStrings> {
        get_static_string(&self.strings, id)
    }

    /// Looks up bytes by their `BytesId`.
    ///
    /// # Panics
    ///
    /// Panics if the `BytesId` is invalid.
    #[inline]
    pub fn get_bytes(&self, id: BytesId) -> &[u8] {
        self.bytes[id.index()].value()
    }

    /// Looks up a long integer by its `LongIntId`.
    ///
    /// # Panics
    ///
    /// Panics if the `LongIntId` is invalid.
    #[inline]
    pub fn get_long_int(&self, id: LongIntId) -> &BigInt {
        self.long_ints[id.index()].value()
    }

    /// Lookup a function by its `FunctionId`
    ///
    /// # Panics
    ///
    /// Panics if the `FunctionId` is invalid.
    #[inline]
    pub fn get_function(&self, id: FunctionId) -> &Function {
        self.functions.get(id.index()).expect("Function not found")
    }

    /// Returns the Python hash for an interned string.
    ///
    /// Reserved-string hashes remain globally lazy. Every executor-local entry, static
    /// or owned, computes and stores its hash once when interned or loaded.
    ///
    /// All three paths must agree with [`hash_python_str`] applied to the
    /// underlying `&str` — interned and heap strings with equal contents
    /// must hash identically.
    ///
    /// # Panics
    ///
    /// Panics if the `StringId` is invalid (same as [`Self::get_str`]).
    #[inline]
    pub fn str_hash(&self, id: StringId) -> HashValue {
        if id.index() < RESERVED_STRS.len() {
            RESERVED_STRING_HASHES.get_or_compute(id.index(), || hash_python_str(RESERVED_STRS[id.index()]))
        } else {
            self.strings[id.index() - INTERN_STRING_ID_OFFSET].hash()
        }
    }

    /// Returns the Python hash for interned bytes.
    ///
    /// Reads the [`HashValue`] from the corresponding [`WithHash`] entry
    /// (populated at intern time). Must agree with [`hash_python_bytes`]
    /// applied to the underlying `&[u8]`.
    ///
    /// # Panics
    ///
    /// Panics if the `BytesId` is invalid.
    #[inline]
    pub fn bytes_hash(&self, id: BytesId) -> HashValue {
        self.bytes[id.index()].hash()
    }

    /// Returns the Python hash for an interned long integer.
    ///
    /// Reads the [`HashValue`] from the corresponding [`WithHash`] entry
    /// (populated at intern time). Must agree with [`hash_python_long_int`].
    /// Note that interned long ints are only created for values that don't
    /// fit in `i64` (see `parse.rs`), so the `to_i64()` fast path inside
    /// `hash_python_long_int` is a defensive consistency guarantee rather
    /// than a hot path.
    ///
    /// # Panics
    ///
    /// Panics if the `LongIntId` is invalid.
    #[inline]
    pub fn long_int_hash(&self, id: LongIntId) -> HashValue {
        self.long_ints[id.index()].hash()
    }

    /// Looks up the executor-local `StringId` for previously interned text.
    ///
    /// This is the reverse of [`Self::get_str`]: given a string, find its
    /// `StringId`. The interned-string branch is O(1) via the
    /// `string_id_by_name` reverse map (built once at construction /
    /// deserialization), so the entire lookup stays O(1) regardless of how
    /// many strings have been interned.
    ///
    /// Used when the host provides a name (e.g., from a `NameLookup` response,
    /// [`MontyRepl::call_function`](crate::MontyRepl::call_function),
    /// [`MontyRepl::has_function`](crate::MontyRepl::has_function), or input
    /// injection) that was previously interned during preparation.
    ///
    /// Returns `None` if the string was never interned.
    pub fn get_string_id_by_name(&self, s: &str) -> Option<StringId> {
        get_string_id_by_name(&self.string_id_by_name, &self.static_string_ids, s)
    }
}
