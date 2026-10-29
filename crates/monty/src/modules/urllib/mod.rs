//! The `urllib` package and its `urllib.parse` module: URL splitting and joining,
//! percent-encoding and query-string handling.
//!
//! `urllib` itself only carries the `parse` submodule, as `import urllib.parse`
//! and `from urllib.parse import ...` load it through the package. The result
//! objects (`ParseResult`, `SplitResult`, `DefragResult`) are class-less named
//! tuples; [`result_attr`] and [`result_geturl`] supply their netloc properties
//! and `geturl()`. Only `str` URLs are parsed; `quote`, `unquote` and the
//! query-string functions take `bytes` as CPython does (see `limitations/urllib.md`).

mod parse;

use std::collections::HashMap;

use smallvec::smallvec;

use self::parse::Split;
use crate::{
    args::{ArgValues, FromArgs, KwargsValues},
    bytecode::VM,
    codecs::Codec,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult},
    heap::{ContainsHeap, DropGuard, DropWithContext, HeapData, HeapId, HeapRead},
    intern::{StaticStrings, StringId},
    modules::ModuleFunctions,
    os_dispatch::value_to_owned_bytes,
    types::{
        Dict, List, Module, NamedTuple, PyTrait, Type,
        bytes::allocate_bytes,
        str::{StringRepr, allocate_string},
        tuple::allocate_tuple,
    },
    value::{EitherStr, Value},
};

/// The functions of `urllib.parse`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum UrllibFunctions {
    Urlparse,
    Urlunparse,
    Urlsplit,
    Urlunsplit,
    Urljoin,
    Urldefrag,
    Quote,
    QuotePlus,
    QuoteFromBytes,
    Unquote,
    UnquotePlus,
    UnquoteToBytes,
    Urlencode,
    ParseQs,
    ParseQsl,
}

/// Creates the `urllib` package, whose only attribute is the `parse` module.
pub fn create_package(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Urllib, vm.interns);
    module.set_attr(StaticStrings::Parse, Value::Ref(create_module(vm)), vm);
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Creates the `urllib.parse` module.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::UrllibParse, vm.interns);
    for (name, function) in [
        (StaticStrings::Urlparse, UrllibFunctions::Urlparse),
        (StaticStrings::Urlunparse, UrllibFunctions::Urlunparse),
        (StaticStrings::Urlsplit, UrllibFunctions::Urlsplit),
        (StaticStrings::Urlunsplit, UrllibFunctions::Urlunsplit),
        (StaticStrings::Urljoin, UrllibFunctions::Urljoin),
        (StaticStrings::Urldefrag, UrllibFunctions::Urldefrag),
        (StaticStrings::Quote, UrllibFunctions::Quote),
        (StaticStrings::QuotePlus, UrllibFunctions::QuotePlus),
        (StaticStrings::QuoteFromBytes, UrllibFunctions::QuoteFromBytes),
        (StaticStrings::Unquote, UrllibFunctions::Unquote),
        (StaticStrings::UnquotePlus, UrllibFunctions::UnquotePlus),
        (StaticStrings::UnquoteToBytes, UrllibFunctions::UnquoteToBytes),
        (StaticStrings::Urlencode, UrllibFunctions::Urlencode),
        (StaticStrings::ParseQs, UrllibFunctions::ParseQs),
        (StaticStrings::ParseQsl, UrllibFunctions::ParseQsl),
    ] {
        module.set_attr(name, Value::ModuleFunction(ModuleFunctions::Urllib(function)), vm);
    }
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to one of the module's functions.
pub(super) fn call(vm: &mut VM<'_>, function: UrllibFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        UrllibFunctions::Urlparse => {
            let UrlparseArgs {
                url,
                scheme,
                allow_fragments,
            } = UrlparseArgs::from_args(args, vm)?;
            defer_drop!(url, vm);
            defer_drop!(scheme, vm);
            defer_drop!(allow_fragments, vm);
            let (url, scheme, allow_fragments) = split_arguments(url, scheme, allow_fragments, vm)?;
            let split = parse::urlsplit(&url, Some(&scheme), allow_fragments)?;
            Ok(parse_result(split, vm))
        }
        UrllibFunctions::Urlsplit => {
            let UrlsplitArgs {
                url,
                scheme,
                allow_fragments,
            } = UrlsplitArgs::from_args(args, vm)?;
            defer_drop!(url, vm);
            defer_drop!(scheme, vm);
            defer_drop!(allow_fragments, vm);
            let (url, scheme, allow_fragments) = split_arguments(url, scheme, allow_fragments, vm)?;
            let split = parse::urlsplit(&url, Some(&scheme), allow_fragments)?;
            Ok(split_result(split, vm))
        }
        UrllibFunctions::Urlunparse => {
            let UrlunparseArgs { components } = UrlunparseArgs::from_args(args, vm)?;
            defer_drop!(components, vm);
            let parts = components_arg(components, 6, vm)?;
            Ok(allocate_string(unparse(&parts), vm.heap))
        }
        UrllibFunctions::Urlunsplit => {
            let UrlunsplitArgs { components } = UrlunsplitArgs::from_args(args, vm)?;
            defer_drop!(components, vm);
            let parts = components_arg(components, 5, vm)?;
            Ok(allocate_string(unsplit(&parts), vm.heap))
        }
        UrllibFunctions::Urljoin => {
            let JoinArgs {
                base,
                url,
                allow_fragments,
            } = JoinArgs::from_args(args, vm)?;
            defer_drop!(base, vm);
            defer_drop!(url, vm);
            defer_drop!(allow_fragments, vm);
            // An empty operand returns the other one unchanged, whatever its type.
            if !base.py_bool(vm)? {
                return Ok(url.clone_with_heap(vm));
            }
            if !url.py_bool(vm)? {
                return Ok(base.clone_with_heap(vm));
            }
            // CPython types the call by `base`; a `str` reference then makes a non-string base a mixed call.
            let base = match Text::extract(base, vm)? {
                Some(Text::Str(base)) => base,
                Some(Text::Bytes(_)) => return Err(bytes_unsupported()),
                None if url.is_str(vm.heap) => return Err(mixed_types()),
                None => return Err(ExcType::attribute_error(base.py_type_name(vm), "decode")),
            };
            let url = mixed_arg(url, vm)?;
            let allow_fragments = allow_fragments.py_bool(vm)?;
            Ok(allocate_string(parse::urljoin(&base, &url, allow_fragments)?, vm.heap))
        }
        UrllibFunctions::Urldefrag => {
            let UrlArgs { url } = UrlArgs::from_args(args, vm)?;
            defer_drop!(url, vm);
            let url = url_arg(url, vm)?;
            let (defrag, fragment) = if url.contains('#') {
                let split = parse::urlsplit(&url, None, true)?;
                let defrag = parse::urlunsplit(
                    split.scheme.as_deref(),
                    split.netloc.as_deref(),
                    &split.path,
                    split.query.as_deref(),
                    None,
                );
                (defrag, split.fragment.unwrap_or_default())
            } else {
                (url, String::new())
            };
            let items = vec![allocate_string(defrag, vm.heap), allocate_string(fragment, vm.heap)];
            Ok(named_result(
                StaticStrings::DefragResult,
                &[StaticStrings::Url, StaticStrings::Fragment],
                items,
                vm,
            ))
        }
        UrllibFunctions::Quote | UrllibFunctions::QuotePlus => {
            let plus = function == UrllibFunctions::QuotePlus;
            let QuoteArgs {
                string,
                safe,
                encoding,
                errors,
            } = if plus {
                QuotePlusArgs::from_args(args, vm)?.into()
            } else {
                QuoteArgs::from_args(args, vm)?
            };
            defer_drop!(string, vm);
            defer_drop!(safe, vm);
            defer_drop!(encoding, vm);
            defer_drop!(errors, vm);
            let quoted = quote(string, safe, encoding, errors, plus, vm)?;
            Ok(allocate_string(quoted, vm.heap))
        }
        UrllibFunctions::QuoteFromBytes => {
            let QuoteFromBytesArgs { bs, safe } = QuoteFromBytesArgs::from_args(args, vm)?;
            defer_drop!(bs, vm);
            defer_drop!(safe, vm);
            let Some(bytes) = value_to_owned_bytes(bs, vm.heap, vm.interns) else {
                return Err(ExcType::type_error("quote_from_bytes() expected bytes"));
            };
            let safe = safe_bytes(safe, vm)?;
            Ok(allocate_string(parse::quote_bytes(&bytes, &safe), vm.heap))
        }
        UrllibFunctions::Unquote => {
            let UnquoteArgs {
                string,
                encoding,
                errors,
            } = UnquoteArgs::from_args(args, vm)?;
            defer_drop!(string, vm);
            defer_drop!(encoding, vm);
            defer_drop!(errors, vm);
            unquote(string, encoding.as_ref(), errors.as_ref(), false, vm)
        }
        UrllibFunctions::UnquotePlus => {
            let UnquotePlusArgs {
                string,
                encoding,
                errors,
            } = UnquotePlusArgs::from_args(args, vm)?;
            defer_drop!(string, vm);
            defer_drop!(encoding, vm);
            defer_drop!(errors, vm);
            unquote(string, encoding.as_ref(), errors.as_ref(), true, vm)
        }
        UrllibFunctions::UnquoteToBytes => {
            let StringArgs { string } = StringArgs::from_args(args, vm)?;
            defer_drop!(string, vm);
            let bytes = match Text::extract(string, vm)? {
                Some(Text::Str(s)) => parse::unquote_bytes(s.as_bytes()),
                Some(Text::Bytes(b)) => parse::unquote_bytes(&b),
                None => return Err(ExcType::attribute_error(string.py_type_name(vm), "split")),
            };
            Ok(allocate_bytes(bytes, vm.heap))
        }
        UrllibFunctions::ParseQs => {
            let args: ParseQsArgs = ParseQsOnlyArgs::from_args(args, vm)?.into();
            defer_drop!(args, vm);
            pairs_dict(parse_qsl(args, vm)?, vm)
        }
        UrllibFunctions::ParseQsl => {
            let args: ParseQsArgs = ParseQslArgs::from_args(args, vm)?.into();
            defer_drop!(args, vm);
            Ok(pairs_list(parse_qsl(args, vm)?, vm))
        }
        UrllibFunctions::Urlencode => {
            let args = UrlencodeArgs::from_args(args, vm)?;
            defer_drop!(args, vm);
            urlencode(args, vm)
        }
    }
}

/// Separate derives keep each function's own name in binding errors.
macro_rules! split_arguments {
    ($ty:ident, $name:literal) => {
        /// Argument shape for `(url, scheme='', allow_fragments=True)`.
        #[derive(FromArgs)]
        #[from_args(name = $name, style = def)]
        struct $ty {
            url: Value,
            #[from_args(default = Value::InternString(StringId::EMPTY))]
            scheme: Value,
            #[from_args(default = Value::Bool(true))]
            allow_fragments: Value,
        }
    };
}
split_arguments!(UrlparseArgs, "urlparse");
split_arguments!(UrlsplitArgs, "urlsplit");

/// Separate derives keep each function's own name in binding errors.
macro_rules! components_arguments {
    ($ty:ident, $name:literal) => {
        /// Argument shape for `(components)`.
        #[derive(FromArgs)]
        #[from_args(name = $name, style = def)]
        struct $ty {
            components: Value,
        }
    };
}
components_arguments!(UrlunparseArgs, "urlunparse");
components_arguments!(UrlunsplitArgs, "urlunsplit");

/// Argument shape for `urljoin(base, url, allow_fragments=True)`.
#[derive(FromArgs)]
#[from_args(name = "urljoin", style = def)]
struct JoinArgs {
    base: Value,
    url: Value,
    #[from_args(default = Value::Bool(true))]
    allow_fragments: Value,
}

/// Argument shape for `urldefrag(url)`.
#[derive(FromArgs)]
#[from_args(name = "urldefrag", style = def)]
struct UrlArgs {
    url: Value,
}

/// Argument shape for `quote(string, safe='/', encoding=None, errors=None)`.
#[derive(FromArgs)]
#[from_args(name = "quote", style = def)]
struct QuoteArgs {
    #[from_args(static_string = "StringAttr")]
    string: Value,
    #[from_args(default = Value::InternString(StringId::from_ascii(b'/')))]
    safe: Value,
    #[from_args(default = Value::None)]
    encoding: Value,
    #[from_args(default = Value::None)]
    errors: Value,
}

/// Argument shape for `quote_plus(string, safe='', encoding=None, errors=None)`.
#[derive(FromArgs)]
#[from_args(name = "quote_plus", style = def)]
struct QuotePlusArgs {
    #[from_args(static_string = "StringAttr")]
    string: Value,
    #[from_args(default = Value::InternString(StringId::EMPTY))]
    safe: Value,
    #[from_args(default = Value::None)]
    encoding: Value,
    #[from_args(default = Value::None)]
    errors: Value,
}

impl From<QuotePlusArgs> for QuoteArgs {
    fn from(args: QuotePlusArgs) -> Self {
        Self {
            string: args.string,
            safe: args.safe,
            encoding: args.encoding,
            errors: args.errors,
        }
    }
}

/// Argument shape for `quote_from_bytes(bs, safe='/')`.
#[derive(FromArgs)]
#[from_args(name = "quote_from_bytes", style = def)]
struct QuoteFromBytesArgs {
    bs: Value,
    #[from_args(default = Value::InternString(StringId::from_ascii(b'/')))]
    safe: Value,
}

/// Separate derives keep each function's own name in binding errors.
macro_rules! unquote_arguments {
    ($ty:ident, $name:literal) => {
        /// Argument shape for `(string, encoding='utf-8', errors='replace')`.
        #[derive(FromArgs)]
        #[from_args(name = $name, style = def)]
        struct $ty {
            #[from_args(static_string = "StringAttr")]
            string: Value,
            // `Option` keeps an omitted argument apart from an explicit `None`,
            // which `bytes.decode()` rejects.
            #[from_args(default)]
            encoding: Option<Value>,
            #[from_args(default)]
            errors: Option<Value>,
        }
    };
}
unquote_arguments!(UnquoteArgs, "unquote");
unquote_arguments!(UnquotePlusArgs, "unquote_plus");

/// Argument shape for `unquote_to_bytes(string)`.
#[derive(FromArgs)]
#[from_args(name = "unquote_to_bytes", style = def)]
struct StringArgs {
    #[from_args(static_string = "StringAttr")]
    string: Value,
}

/// Separate derives keep each function's own name in binding errors; both
/// convert into [`ParseQsArgs`].
macro_rules! parse_qs_arguments {
    ($ty:ident, $name:literal) => {
        /// Argument shape for
        /// `(qs, keep_blank_values=False, strict_parsing=False, encoding='utf-8', errors='replace', max_num_fields=None, separator='&')`.
        #[derive(FromArgs)]
        #[from_args(name = $name, style = def)]
        struct $ty {
            qs: Value,
            #[from_args(default = Value::Bool(false))]
            keep_blank_values: Value,
            #[from_args(default = Value::Bool(false))]
            strict_parsing: Value,
            #[from_args(default = Value::None)]
            encoding: Value,
            #[from_args(default = Value::None)]
            errors: Value,
            #[from_args(default = Value::None)]
            max_num_fields: Value,
            #[from_args(default = Value::InternString(StringId::from_ascii(b'&')))]
            separator: Value,
        }

        impl From<$ty> for ParseQsArgs {
            fn from(args: $ty) -> Self {
                Self {
                    qs: args.qs,
                    keep_blank_values: args.keep_blank_values,
                    strict_parsing: args.strict_parsing,
                    encoding: args.encoding,
                    errors: args.errors,
                    max_num_fields: args.max_num_fields,
                    separator: args.separator,
                }
            }
        }
    };
}
parse_qs_arguments!(ParseQsOnlyArgs, "parse_qs");
parse_qs_arguments!(ParseQslArgs, "parse_qsl");

/// The bound `parse_qs` / `parse_qsl` arguments.
struct ParseQsArgs {
    qs: Value,
    keep_blank_values: Value,
    strict_parsing: Value,
    encoding: Value,
    errors: Value,
    max_num_fields: Value,
    separator: Value,
}

impl<C: ContainsHeap> DropWithContext<C> for ParseQsArgs {
    fn drop_with(self, ctx: &mut C) {
        self.qs.drop_with(ctx);
        self.keep_blank_values.drop_with(ctx);
        self.strict_parsing.drop_with(ctx);
        self.encoding.drop_with(ctx);
        self.errors.drop_with(ctx);
        self.max_num_fields.drop_with(ctx);
        self.separator.drop_with(ctx);
    }
}

/// Argument shape for `urlencode(query, doseq=False, safe='', encoding=None, errors=None, quote_via=quote_plus)`.
#[derive(FromArgs)]
#[from_args(name = "urlencode", style = def)]
struct UrlencodeArgs {
    query: Value,
    #[from_args(default = Value::Bool(false))]
    doseq: Value,
    #[from_args(default = Value::InternString(StringId::EMPTY))]
    safe: Value,
    #[from_args(default = Value::None)]
    encoding: Value,
    #[from_args(default = Value::None)]
    errors: Value,
    #[from_args(default = Value::ModuleFunction(ModuleFunctions::Urllib(UrllibFunctions::QuotePlus)))]
    quote_via: Value,
}

impl<C: ContainsHeap> DropWithContext<C> for UrlencodeArgs {
    fn drop_with(self, ctx: &mut C) {
        self.query.drop_with(ctx);
        self.doseq.drop_with(ctx);
        self.safe.drop_with(ctx);
        self.encoding.drop_with(ctx);
        self.errors.drop_with(ctx);
        self.quote_via.drop_with(ctx);
    }
}

/// The `urlsplit` / `urlparse` arguments as Rust values.
fn split_arguments(
    url: &Value,
    scheme: &Value,
    allow_fragments: &Value,
    vm: &mut VM<'_>,
) -> RunResult<(String, String, bool)> {
    let url = url_arg(url, vm)?;
    let scheme = if scheme.is_str(vm.heap) {
        scheme.to_str(vm)?.to_owned()
    } else if scheme.py_bool(vm)? {
        return Err(mixed_types());
    } else {
        // CPython strips the default scheme, so a falsy non-string fails there.
        return Err(ExcType::attribute_error(scheme.py_type_name(vm), "strip"));
    };
    Ok((url, scheme, allow_fragments.py_bool(vm)?))
}

/// A URL argument: `str` only. `bytes` URLs are not supported, and any other
/// type fails as CPython's bytes coercion fails on it.
fn url_arg(value: &Value, vm: &mut VM<'_>) -> RunResult<String> {
    match Text::extract(value, vm)? {
        Some(Text::Str(url)) => Ok(url),
        Some(Text::Bytes(_)) => Err(bytes_unsupported()),
        None => Err(ExcType::attribute_error(value.py_type_name(vm), "decode")),
    }
}

/// A second string argument, which CPython checks against the first one's type:
/// a truthy non-`str` is a mixed call.
fn mixed_arg(value: &Value, vm: &mut VM<'_>) -> RunResult<String> {
    match Text::extract(value, vm)? {
        Some(Text::Str(text)) => Ok(text),
        Some(Text::Bytes(_)) => Err(bytes_unsupported()),
        None => Err(mixed_types()),
    }
}

/// The items of a `urlunsplit` / `urlunparse` components iterable, as `expected`
/// strings. A falsy non-string (`None`) counts as empty, a truthy one is a mixed call.
fn components_arg(components: &Value, expected: usize, vm: &mut VM<'_>) -> RunResult<Vec<String>> {
    let iter = match components.clone_with_heap(vm).into_py_iter(vm) {
        Ok(iter) => iter.require_synchronous(vm)?,
        // CPython fails star-unpacking the components into its coercion helper.
        Err(RunError::Exc(exc)) if exc.exc.exc_type() == ExcType::TypeError => {
            return Err(ExcType::type_error(format!(
                "urllib.parse._coerce_args() argument after * must be an iterable, not {}",
                components.py_type_name(vm)
            )));
        }
        Err(error) => return Err(error),
    };
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);
    let mut parts = Vec::with_capacity(expected);
    while let Some(item) = iter.py_next(vm)? {
        defer_drop!(item, vm);
        let part = match Text::extract(item, vm)? {
            Some(Text::Str(text)) => text,
            Some(Text::Bytes(_)) => return Err(bytes_unsupported()),
            None if item.py_bool(vm)? => return Err(mixed_types()),
            None => String::new(),
        };
        parts.push(part);
    }
    // CPython unpacks the components together with its result converter.
    if parts.len() != expected {
        return Err(ExcType::unpack_size_error(expected + 1, parts.len() + 1));
    }
    Ok(parts)
}

/// `urlunsplit` on five component strings.
fn unsplit(parts: &[String]) -> String {
    let [scheme, netloc, path, query, fragment] = parts else {
        unreachable!("components_arg checked the count")
    };
    parse::urlunsplit(
        Some(scheme),
        parse::netloc_for_unsplit(scheme, netloc, path),
        path,
        Some(query.as_str()).filter(|q| !q.is_empty()),
        Some(fragment.as_str()).filter(|f| !f.is_empty()),
    )
}

/// `urlunparse` on six component strings: the params rejoin the path first.
fn unparse(parts: &[String]) -> String {
    let [scheme, netloc, path, params, query, fragment] = parts else {
        unreachable!("components_arg checked the count")
    };
    let path = if params.is_empty() {
        path.clone()
    } else {
        format!("{path};{params}")
    };
    parse::urlunsplit(
        Some(scheme),
        parse::netloc_for_unsplit(scheme, netloc, &path),
        &path,
        Some(query.as_str()).filter(|q| !q.is_empty()),
        Some(fragment.as_str()).filter(|f| !f.is_empty()),
    )
}

/// Builds a `SplitResult` from a split.
fn split_result(split: Split, vm: &mut VM<'_>) -> Value {
    let items = vec![
        allocate_string(split.scheme.unwrap_or_default(), vm.heap),
        allocate_string(split.netloc.unwrap_or_default(), vm.heap),
        allocate_string(split.path, vm.heap),
        allocate_string(split.query.unwrap_or_default(), vm.heap),
        allocate_string(split.fragment.unwrap_or_default(), vm.heap),
    ];
    named_result(
        StaticStrings::SplitResult,
        &[
            StaticStrings::Scheme,
            StaticStrings::Netloc,
            StaticStrings::Path,
            StaticStrings::Query,
            StaticStrings::Fragment,
        ],
        items,
        vm,
    )
}

/// Builds a `ParseResult` from a split, separating the path's params.
fn parse_result(split: Split, vm: &mut VM<'_>) -> Value {
    let scheme = split.scheme.unwrap_or_default();
    let (path, params) = parse::split_params(&scheme, &split.path).unwrap_or((split.path, String::new()));
    let items = vec![
        allocate_string(scheme, vm.heap),
        allocate_string(split.netloc.unwrap_or_default(), vm.heap),
        allocate_string(path, vm.heap),
        allocate_string(params, vm.heap),
        allocate_string(split.query.unwrap_or_default(), vm.heap),
        allocate_string(split.fragment.unwrap_or_default(), vm.heap),
    ];
    named_result(
        StaticStrings::ParseResult,
        &[
            StaticStrings::Scheme,
            StaticStrings::Netloc,
            StaticStrings::Path,
            StaticStrings::Params,
            StaticStrings::Query,
            StaticStrings::Fragment,
        ],
        items,
        vm,
    )
}

/// Allocates one of the result named tuples. They have no class object, which
/// is what marks them for [`result_attr`] and [`result_geturl`].
fn named_result(name: StaticStrings, fields: &[StaticStrings], items: Vec<Value>, vm: &mut VM<'_>) -> Value {
    let named_tuple = NamedTuple::new(
        EitherStr::Interned(vm.interns.intern_static(name)),
        fields
            .iter()
            .map(|field| EitherStr::Interned(vm.interns.intern_static(*field)))
            .collect(),
        items,
    );
    Value::Ref(vm.heap.allocate(HeapData::NamedTuple(Box::new(named_tuple))))
}

/// Which result named tuple a class-less named tuple is, by name.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ResultKind {
    Parse,
    Split,
    Defrag,
}

impl ResultKind {
    /// The kind a class-less named tuple is, by name and field count: a
    /// `collections.namedtuple('SplitResult', ...)` of another shape is not one.
    fn of(named_tuple: &NamedTuple, vm: &VM<'_>) -> Option<Self> {
        let kind = match named_tuple.name_either().static_string(vm.interns)? {
            StaticStrings::ParseResult => Self::Parse,
            StaticStrings::SplitResult => Self::Split,
            StaticStrings::DefragResult => Self::Defrag,
            _ => return None,
        };
        (named_tuple.items().len() == kind.field_count()).then_some(kind)
    }

    /// How many fields the result has.
    fn field_count(self) -> usize {
        match self {
            Self::Parse => 6,
            Self::Split => 5,
            Self::Defrag => 2,
        }
    }
}

/// Whether a class-less named tuple is one of the `urllib.parse` results, which
/// carry the `collections.namedtuple` API (`_fields`, `_replace`, ...) in CPython.
pub(crate) fn is_result(named_tuple: &NamedTuple, vm: &VM<'_>) -> bool {
    ResultKind::of(named_tuple, vm).is_some()
}

/// The netloc properties of a `ParseResult` / `SplitResult` (`hostname`, `port`,
/// `username`, `password`), for the named-tuple attribute lookup. `None` when the
/// tuple is not a URL result or the attribute is not one of them.
pub(crate) fn result_attr<'h>(
    named_tuple: &HeapRead<'h, NamedTuple>,
    attr: &EitherStr,
    vm: &mut VM<'h>,
) -> RunResult<Option<Value>> {
    let tuple = named_tuple.get(vm.heap);
    if !matches!(ResultKind::of(tuple, vm), Some(ResultKind::Parse | ResultKind::Split)) {
        return Ok(None);
    }
    let Some(
        attr @ (StaticStrings::Hostname | StaticStrings::Port | StaticStrings::Username | StaticStrings::Password),
    ) = attr.static_string(vm.interns)
    else {
        return Ok(None);
    };
    // Only the netloc properties read the netloc, so `_replace(netloc=None).path` still works.
    let netloc = tuple.items()[1].to_str(vm)?.to_owned();
    let optional =
        |text: Option<&str>, vm: &mut VM<'_>| text.map_or(Value::None, |text| allocate_string(text, vm.heap));
    Ok(Some(match attr {
        StaticStrings::Hostname => optional(parse::hostname(&netloc).as_deref(), vm),
        StaticStrings::Port => parse::port(&netloc)?.map_or(Value::None, Value::Int),
        StaticStrings::Username => optional(parse::userinfo(&netloc).0, vm),
        _ => optional(parse::userinfo(&netloc).1, vm),
    }))
}

/// `geturl()` on a result named tuple: the URL reassembled from its parts.
/// `None` when the tuple is not a URL result.
pub(crate) fn result_geturl<'h>(
    named_tuple: &HeapRead<'h, NamedTuple>,
    args: ArgValues,
    vm: &mut VM<'h>,
) -> RunResult<Option<Value>> {
    let count = args.count();
    let (positional, kwargs) = args.into_parts();
    defer_drop!(positional, vm);
    defer_drop!(kwargs, vm);
    let tuple = named_tuple.get(vm.heap);
    let Some(kind) = ResultKind::of(tuple, vm) else {
        return Ok(None);
    };
    let name = tuple.name(vm.interns).to_owned();
    let parts: Vec<String> = tuple
        .items()
        .iter()
        .map(|item| item.to_str(vm).map(str::to_owned))
        .collect::<RunResult<_>>()?;
    if let Some(key) = kwargs.first_key(vm.heap, vm.interns)? {
        return Err(ExcType::type_error_unexpected_keyword(&format!("{name}.geturl"), &key));
    }
    if count > 0 {
        return Err(ExcType::type_error(format!(
            "{name}.geturl() takes 1 positional argument but {} were given",
            count + 1
        )));
    }
    let url = match kind {
        ResultKind::Parse => unparse(&parts),
        ResultKind::Split => unsplit(&parts),
        ResultKind::Defrag => {
            if parts[1].is_empty() {
                parts[0].clone()
            } else {
                format!("{}#{}", parts[0], parts[1])
            }
        }
    };
    Ok(Some(allocate_string(url, vm.heap)))
}

/// `quote()` / `quote_plus()`: a `str` is encoded first (UTF-8 and `strict` by
/// default), `bytes` are quoted as they are and refuse `encoding` / `errors`.
fn quote(
    string: &Value,
    safe: &Value,
    encoding: &Value,
    errors: &Value,
    plus: bool,
    vm: &mut VM<'_>,
) -> RunResult<String> {
    let bytes = match Text::extract(string, vm)? {
        Some(Text::Str(s)) => {
            if s.is_empty() {
                return Ok(String::new());
            }
            let encoding = codec_arg(encoding, Some("utf-8"), "encode", "encoding", vm)?;
            let errors = codec_arg(errors, Some("strict"), "encode", "errors", vm)?;
            let codec = Codec::find(&encoding).ok_or_else(|| ExcType::lookup_error_unknown_encoding(&encoding))?;
            codec.encode(&s, &errors, &vm.heap.tracker)?
        }
        other => {
            // Anything but a `str` takes the bytes path, whose codec checks come first.
            if !matches!(encoding, Value::None) {
                return Err(ExcType::type_error("quote() doesn't support 'encoding' for bytes"));
            }
            if !matches!(errors, Value::None) {
                return Err(ExcType::type_error("quote() doesn't support 'errors' for bytes"));
            }
            match other {
                Some(Text::Bytes(bytes)) => bytes,
                _ => return Err(ExcType::type_error("quote_from_bytes() expected bytes")),
            }
        }
    };
    let mut safe = safe_bytes(safe, vm)?;
    // `quote_plus` lets spaces through quoting so it can turn them into `+`.
    let plus = plus && bytes.contains(&b' ');
    if plus {
        safe.push(b' ');
    }
    let quoted = parse::quote_bytes(&bytes, &safe);
    Ok(if plus { quoted.replace(' ', "+") } else { quoted })
}

/// The `safe` characters as ASCII bytes: non-ASCII characters of a `str`, and
/// codes past 127 of `bytes` or any other iterable of ints, are dropped as CPython
/// drops them.
fn safe_bytes(safe: &Value, vm: &mut VM<'_>) -> RunResult<Vec<u8>> {
    match Text::extract(safe, vm)? {
        Some(Text::Str(s)) => Ok(s.bytes().filter(u8::is_ascii).collect()),
        Some(Text::Bytes(b)) => Ok(b.into_iter().filter(u8::is_ascii).collect()),
        None => {
            let iter = safe.clone_with_heap(vm).into_py_iter(vm)?.require_synchronous(vm)?;
            defer_drop!(iter, vm);
            let mut iter = iter.read(vm);
            let mut out = Vec::new();
            while let Some(item) = iter.py_next(vm)? {
                defer_drop!(item, vm);
                // `c < 128` first, then `bytes([...])`, with each step's CPython error.
                let code = match item {
                    Value::Int(code) => *code,
                    Value::Bool(flag) => i64::from(*flag),
                    Value::Float(_) => item.as_int(vm)?,
                    other => return Err(ExcType::type_error_ordering("<", &other.py_type_name(vm), "int")),
                };
                if code < 0 {
                    return Err(ExcType::value_error("bytes must be in range(0, 256)"));
                }
                if let Ok(code) = u8::try_from(code)
                    && code.is_ascii()
                {
                    out.push(code);
                }
            }
            Ok(out)
        }
    }
}

/// An `encoding` / `errors` argument of `str.encode()` / `bytes.decode()`:
/// `None` means `default` where the caller has one, otherwise it must be a `str`.
fn codec_arg(value: &Value, default: Option<&str>, method: &str, name: &str, vm: &mut VM<'_>) -> RunResult<String> {
    match (value, default) {
        (Value::None, Some(default)) => Ok(default.to_owned()),
        (value, _) if value.is_str(vm.heap) => Ok(value.to_str(vm)?.to_owned()),
        // `_PyArg_BadArgument` writes `None` itself rather than `NoneType`.
        (Value::None, None) => Err(ExcType::type_error(format!(
            "{method}() argument '{name}' must be str, not None"
        ))),
        (value, _) => Err(ExcType::type_error(format!(
            "{method}() argument '{name}' must be str, not {}",
            value.py_type_name(vm)
        ))),
    }
}

/// `unquote()` / `unquote_plus()`: `%XX` escapes decode with `encoding` and
/// `errors` (UTF-8 and `replace` by default); a `str` without `%` is returned as is.
fn unquote(
    string: &Value,
    encoding: Option<&Value>,
    errors: Option<&Value>,
    plus: bool,
    vm: &mut VM<'_>,
) -> RunResult<Value> {
    match Text::extract(string, vm)? {
        Some(Text::Bytes(bytes)) => {
            if plus {
                // CPython's `unquote_plus` replaces with a `str` pattern first.
                return Err(ExcType::type_error("a bytes-like object is required, not 'str'"));
            }
            // `bytes.decode(encoding, errors)` takes the arguments as given, so an
            // explicit `None` is rejected where an omitted one means the default.
            let encoding = match encoding {
                Some(value) => codec_arg(value, None, "decode", "encoding", vm)?,
                None => "utf-8".to_owned(),
            };
            let errors = match errors {
                Some(value) => codec_arg(value, None, "decode", "errors", vm)?,
                None => "replace".to_owned(),
            };
            let codec = Codec::find(&encoding).ok_or_else(|| ExcType::lookup_error_unknown_encoding(&encoding))?;
            let decoded = codec.decode(&parse::unquote_bytes(&bytes), &errors)?;
            Ok(allocate_string(decoded, vm.heap))
        }
        Some(Text::Str(s)) => {
            let encoding = codec_arg(
                encoding.unwrap_or(&Value::None),
                Some("utf-8"),
                "decode",
                "encoding",
                vm,
            )?;
            let errors = codec_arg(errors.unwrap_or(&Value::None), Some("replace"), "decode", "errors", vm)?;
            let codec = Codec::find(&encoding).ok_or_else(|| ExcType::lookup_error_unknown_encoding(&encoding))?;
            let s = if plus { s.replace('+', " ") } else { s };
            if !s.contains('%') {
                return Ok(if plus {
                    allocate_string(s, vm.heap)
                } else {
                    string.clone_with_heap(vm)
                });
            }
            Ok(allocate_string(unquote_str(&s, codec, &errors)?, vm.heap))
        }
        // CPython fails its `'%' not in string` test.
        None => Err(ExcType::type_error(format!(
            "argument of type '{}' is not a container or iterable",
            string.py_type_name(vm)
        ))),
    }
}

/// Percent-decodes the ASCII runs of `s` with `codec`, keeping every other
/// character as it is, as CPython's `unquote()` does for a `str`.
fn unquote_str(s: &str, codec: Codec, errors: &str) -> RunResult<String> {
    let mut out = String::with_capacity(s.len());
    for (ascii, run) in parse::ascii_runs(s) {
        if ascii {
            out.push_str(&codec.decode(&parse::unquote_bytes(run.as_bytes()), errors)?);
        } else {
            out.push_str(run);
        }
    }
    Ok(out)
}

/// `parse_qsl`: the `(name, value)` pairs of a query string, percent-decoded.
/// A `str` query gives `str` pairs, a `bytes` query `bytes` pairs.
fn parse_qsl(args: &ParseQsArgs, vm: &mut VM<'_>) -> RunResult<Vec<(Text, Text)>> {
    let separator = match Text::extract(&args.separator, vm)? {
        Some(separator) if !separator.is_empty() => separator,
        _ => return Err(ExcType::value_error("Separator must be of type string or bytes.")),
    };
    let qs = match Text::extract(&args.qs, vm)? {
        Some(qs) => qs,
        None if matches!(args.qs, Value::None) || !args.qs.py_bool(vm)? => return Ok(Vec::new()),
        None => {
            return Err(ExcType::type_error(format!(
                "memoryview: a bytes-like object is required, not '{}'",
                args.qs.py_type_name(vm)
            )));
        }
    };
    if qs.is_empty() {
        return Ok(Vec::new());
    }
    let keep_blank_values = args.keep_blank_values.py_bool(vm)?;
    let strict_parsing = args.strict_parsing.py_bool(vm)?;
    let is_bytes = qs.is_bytes();
    // A `bytes` query never decodes, so its codec arguments go unread, as in CPython.
    let codec = if is_bytes {
        None
    } else {
        let encoding = codec_arg(&args.encoding, Some("utf-8"), "decode", "encoding", vm)?;
        let errors = codec_arg(&args.errors, Some("replace"), "decode", "errors", vm)?;
        let codec = Codec::find(&encoding).ok_or_else(|| ExcType::lookup_error_unknown_encoding(&encoding))?;
        Some((codec, errors))
    };

    let qs = qs.into_bytes();
    let separator = separator.into_bytes();
    let fields: Vec<&[u8]> = split_bytes(&qs, &separator);
    if !matches!(args.max_num_fields, Value::None)
        && args.max_num_fields.as_int(vm)? < i64::try_from(fields.len()).unwrap_or(i64::MAX)
    {
        return Err(ExcType::value_error("Max number of fields exceeded"));
    }

    let mut pairs = Vec::new();
    for field in fields {
        if field.is_empty() && !strict_parsing {
            continue;
        }
        let (name, value) = match field.iter().position(|&b| b == b'=') {
            Some(eq) => (&field[..eq], &field[eq + 1..]),
            None if strict_parsing => {
                return Err(ExcType::value_error(format!(
                    "bad query field: {}",
                    StringRepr(&String::from_utf8_lossy(field))
                )));
            }
            None => (field, &field[field.len()..]),
        };
        if value.is_empty() && !keep_blank_values {
            continue;
        }
        let decode = |part: &[u8]| -> RunResult<Text> {
            let plus_replaced: Vec<u8> = part.iter().map(|&b| if b == b'+' { b' ' } else { b }).collect();
            match &codec {
                None => Ok(Text::Bytes(parse::unquote_bytes(&plus_replaced))),
                // The split points are ASCII, so the field is still valid UTF-8.
                Some((codec, errors)) => Ok(Text::Str(unquote_str(
                    &String::from_utf8_lossy(&plus_replaced),
                    *codec,
                    errors,
                )?)),
            }
        };
        pairs.push((decode(name)?, decode(value)?));
    }
    Ok(pairs)
}

/// Splits `bytes` on every occurrence of `separator`, like `bytes.split(sep)`.
fn split_bytes<'a>(bytes: &'a [u8], separator: &[u8]) -> Vec<&'a [u8]> {
    let mut fields = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i + separator.len() <= bytes.len() {
        if &bytes[i..i + separator.len()] == separator {
            fields.push(&bytes[start..i]);
            i += separator.len();
            start = i;
        } else {
            i += 1;
        }
    }
    fields.push(&bytes[start..]);
    fields
}

/// `parse_qsl`'s list of two-tuples.
fn pairs_list(pairs: Vec<(Text, Text)>, vm: &mut VM<'_>) -> Value {
    let items = pairs
        .into_iter()
        .map(|(name, value)| allocate_tuple(smallvec![name.allocate(vm), value.allocate(vm)], vm.heap))
        .collect();
    Value::Ref(vm.heap.allocate(HeapData::List(List::new(items))))
}

/// `parse_qs`'s dict of lists, in first-seen order.
fn pairs_dict(pairs: Vec<(Text, Text)>, vm: &mut VM<'_>) -> RunResult<Value> {
    // Keys are found through a map so many distinct keys stay linear.
    let mut grouped: Vec<(Text, Vec<Text>)> = Vec::new();
    let mut positions: HashMap<Text, usize> = HashMap::new();
    for (name, value) in pairs {
        if let Some(&index) = positions.get(&name) {
            grouped[index].1.push(value);
        } else {
            positions.insert(name.clone(), grouped.len());
            grouped.push((name, vec![value]));
        }
    }
    let mut dict_guard = DropGuard::new(Dict::new(), vm);
    let (dict, vm) = dict_guard.as_parts_mut();
    for (name, values) in grouped {
        let items = values.into_iter().map(|value| value.allocate(vm)).collect();
        let list = Value::Ref(vm.heap.allocate(HeapData::List(List::new(items))));
        let key = name.allocate(vm);
        if let Some(previous) = dict.set(key, list, vm)? {
            previous.drop_with(vm);
        }
    }
    let (dict, vm) = dict_guard.into_parts();
    Ok(Value::Ref(vm.heap.allocate(HeapData::Dict(dict))))
}

/// `urlencode()`: `key=value` pairs joined with `&`, each part quoted by
/// `quote_via` (`bytes` as they are, anything else through `str()`).
fn urlencode(args: &UrlencodeArgs, vm: &mut VM<'_>) -> RunResult<Value> {
    let pairs = query_pairs(&args.query, vm)?;
    defer_drop!(pairs, vm);
    let doseq = args.doseq.py_bool(vm)?;
    let mut parts: Vec<String> = Vec::new();
    for (key, value) in pairs {
        let key = quote_via(args, key, true, vm)?;
        if doseq
            && Text::extract(value, vm)?.is_none()
            && let Some(_) = value.py_len(vm)
        {
            let iter = value.clone_with_heap(vm).into_py_iter(vm)?.require_synchronous(vm)?;
            defer_drop!(iter, vm);
            let mut iter = iter.read(vm);
            while let Some(element) = iter.py_next(vm)? {
                defer_drop!(element, vm);
                parts.push(format!("{key}={}", quote_via(args, element, false, vm)?));
            }
        } else {
            parts.push(format!("{key}={}", quote_via(args, value, false, vm)?));
        }
    }
    Ok(allocate_string(parts.join("&"), vm.heap))
}

/// The `(key, value)` pairs of a `urlencode` query: a mapping's items, or a
/// sequence of two-tuples.
fn query_pairs(query: &Value, vm: &mut VM<'_>) -> RunResult<Vec<(Value, Value)>> {
    if let Value::Ref(id) = query
        && let HeapData::Dict(dict) = vm.heap.get(*id)
    {
        return Ok(dict
            .iter()
            .map(|(k, v)| (k.clone_with_heap(vm.heap), v.clone_with_heap(vm.heap)))
            .collect());
    }
    let not_a_sequence = || ExcType::type_error("not a valid non-string sequence or mapping object");
    let Some(len) = query.py_len(vm) else {
        return Err(not_a_sequence());
    };
    let iter = query.clone_with_heap(vm).into_py_iter(vm)?.require_synchronous(vm)?;
    defer_drop!(iter, vm);
    let mut iter = iter.read(vm);
    let mut pairs_guard = DropGuard::new(Vec::<(Value, Value)>::new(), vm);
    let (pairs, vm) = pairs_guard.as_parts_mut();
    while let Some(item) = iter.py_next(vm)? {
        defer_drop!(item, vm);
        let is_tuple =
            matches!(item, Value::Ref(id) if matches!(vm.heap.get(*id), HeapData::Tuple(_) | HeapData::NamedTuple(_)));
        if len > 0 && pairs.is_empty() && !is_tuple {
            return Err(not_a_sequence());
        }
        let pair_iter = item.clone_with_heap(vm).into_py_iter(vm)?.require_synchronous(vm)?;
        defer_drop!(pair_iter, vm);
        let mut pair_iter = pair_iter.read(vm);
        let mut members_guard = DropGuard::new(Vec::<Value>::new(), vm);
        let (members, vm) = members_guard.as_parts_mut();
        while let Some(member) = pair_iter.py_next(vm)? {
            members.push(member);
        }
        if members.len() != 2 {
            return Err(ExcType::unpack_size_error(2, members.len()));
        }
        let mut members = members_guard.into_inner();
        let value = members.pop().expect("two members");
        let key = members.pop().expect("two members");
        pairs.push((key, value));
    }
    Ok(pairs_guard.into_inner())
}

/// Quotes one `urlencode` part with `quote_via`: `bytes` get `(part, safe)`,
/// anything else `(str(part), safe, encoding, errors)`. The module's own
/// functions run natively; another callable is called and must return `str`,
/// or the join with `=` fails as it does in CPython (`is_key` picks the message).
fn quote_via(args: &UrlencodeArgs, part: &Value, is_key: bool, vm: &mut VM<'_>) -> RunResult<String> {
    let is_bytes = part.py_type_heap(vm.heap) == Type::Bytes;
    let text = if is_bytes {
        part.clone_with_heap(vm)
    } else {
        part.py_str(vm)?
    };
    defer_drop!(text, vm);
    match args.quote_via {
        Value::ModuleFunction(ModuleFunctions::Urllib(
            function @ (UrllibFunctions::Quote | UrllibFunctions::QuotePlus),
        )) => {
            let (encoding, errors) = if is_bytes {
                (&Value::None, &Value::None)
            } else {
                (&args.encoding, &args.errors)
            };
            quote(
                text,
                &args.safe,
                encoding,
                errors,
                function == UrllibFunctions::QuotePlus,
                vm,
            )
        }
        ref callable => {
            let call_args = if is_bytes {
                ArgValues::Two(text.clone_with_heap(vm), args.safe.clone_with_heap(vm))
            } else {
                ArgValues::from_parts(
                    vec![
                        text.clone_with_heap(vm),
                        args.safe.clone_with_heap(vm),
                        args.encoding.clone_with_heap(vm),
                        args.errors.clone_with_heap(vm),
                    ],
                    KwargsValues::Empty,
                )
            };
            let quoted = vm.evaluate_function(callable, call_args)?;
            defer_drop!(quoted, vm);
            if quoted.is_str(vm.heap) {
                Ok(quoted.to_str(vm)?.to_owned())
            } else if is_key {
                Err(ExcType::type_error(format!(
                    "unsupported operand type(s) for +: '{}' and 'str'",
                    quoted.py_type_name(vm)
                )))
            } else {
                Err(ExcType::type_error(format!(
                    "can only concatenate str (not \"{}\") to str",
                    quoted.py_type_name(vm)
                )))
            }
        }
    }
}

/// A `str` or `bytes` argument, copied out of the heap.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Text {
    Str(String),
    Bytes(Vec<u8>),
}

impl Text {
    /// `str` or `bytes` text, else `None`.
    fn extract(value: &Value, vm: &VM<'_>) -> RunResult<Option<Self>> {
        if value.is_str(vm.heap) {
            Ok(Some(Self::Str(value.to_str(vm)?.to_owned())))
        } else {
            Ok(value_to_owned_bytes(value, vm.heap, vm.interns).map(Self::Bytes))
        }
    }

    fn is_bytes(&self) -> bool {
        matches!(self, Self::Bytes(_))
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::Str(s) => s.is_empty(),
            Self::Bytes(b) => b.is_empty(),
        }
    }

    /// The text as bytes: `str` as UTF-8.
    fn into_bytes(self) -> Vec<u8> {
        match self {
            Self::Str(s) => s.into_bytes(),
            Self::Bytes(b) => b,
        }
    }

    /// Allocates the text as the type it arrived with.
    fn allocate(self, vm: &mut VM<'_>) -> Value {
        match self {
            Self::Str(s) => allocate_string(s, vm.heap),
            Self::Bytes(b) => allocate_bytes(b, vm.heap),
        }
    }
}

fn mixed_types() -> RunError {
    ExcType::type_error("Cannot mix str and non-str arguments")
}

fn bytes_unsupported() -> RunError {
    ExcType::type_error("urllib.parse does not support bytes URLs; decode to str first")
}
