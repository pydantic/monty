//! Python `pathlib.Path` type implementation.
//!
//! Provides a path object with both pure methods (no I/O) and filesystem methods
//! (require `OsAccess` implementation). Pure methods are handled directly by the VM,
//! while filesystem methods yield external function calls for the host to resolve.

use std::{
    cell::Cell,
    collections::hash_map::DefaultHasher,
    fmt::Write,
    hash::{Hash, Hasher},
};

use monty_types::{MontyPath, ResourceTracker, scan::SegmentPattern};
use smallvec::SmallVec;

use crate::{
    args::{ArgValues, FromArgs, LaxBool},
    builtins::open::builtin_open,
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    hash::HashValue,
    heap::{DropWithContext, Heap, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput},
    intern::{Interns, StaticStrings},
    os_dispatch::{build_path_os_call, is_path_os_method, value_as_path_str},
    types::{
        LazyHeapSet, List, PyTrait, Type, allocate_tuple,
        dir_scan::{glob_call, walk_call},
        str::{StringRepr, allocate_string},
    },
    value::{EitherStr, Value},
};

/// Python `pathlib.Path` object representing a filesystem path.
///
/// Stores a normalized POSIX path string. Windows-style paths are converted
/// to POSIX format (backslashes to forward slashes).
///
/// The path is immutable - all operations that would modify the path return
/// new `Path` objects or strings.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Path {
    /// The normalized path string.
    path: String,
    /// Lazily-computed Python hash. Paths are immutable (all "modifying"
    /// operations return new `Path` objects). Skipped on serde — see
    /// [`super::Str::cached_hash`] for the rationale.
    #[serde(skip)]
    cached_hash: Cell<Option<HashValue>>,
}

impl PartialEq for Path {
    /// Compares only the path string — `cached_hash` is a pure optimisation.
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}

impl Path {
    /// Creates a new `Path` from a path string.
    ///
    /// The path is normalized:
    /// - Backslashes are converted to forward slashes
    /// - Trailing slashes are preserved for root paths only
    #[must_use]
    pub fn new(path: String) -> Self {
        Self {
            path: normalize_path(path),
            cached_hash: Cell::new(None),
        }
    }

    /// Returns the path as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.path
    }

    /// Returns the final component of the path.
    ///
    /// Returns an empty string for `.`, the root, or an empty path.
    #[must_use]
    pub fn name(&self) -> &str {
        if self.path == "." {
            ""
        } else {
            self.path.rsplit_once('/').map_or(self.path.as_str(), |(_, name)| name)
        }
    }

    /// Returns the path without its final component (parent directory).
    ///
    /// For relative paths without a directory (like `file.txt`), returns `.`.
    /// Returns `None` only for the root path `/`.
    #[must_use]
    pub fn parent(&self) -> Option<&str> {
        if self.path == "/" {
            return None;
        }
        match self.path.rsplit_once('/') {
            Some((parent, _)) => Some(if parent.is_empty() { "/" } else { parent }),
            None => Some("."), // Relative path without directory component
        }
    }

    /// Returns the final component without its last suffix.
    ///
    /// If the name has multiple suffixes (e.g., "file.tar.gz"), only the
    /// last suffix is removed.
    #[must_use]
    pub fn stem(&self) -> &str {
        split_suffix(self.name()).0
    }

    /// Returns the file extension (last suffix), including the leading dot.
    ///
    /// Returns an empty string if there is no extension.
    #[must_use]
    pub fn suffix(&self) -> &str {
        split_suffix(self.name()).1
    }

    /// Returns all file extensions as a list of strings.
    ///
    /// Each suffix includes its leading dot. As in CPython, leading dots are
    /// stripped first, so `.bashrc` and `...` have none while `a.` has `['.']`.
    #[must_use]
    pub fn suffixes(&self) -> Vec<&str> {
        let mut rest = self.name().trim_start_matches('.');
        let mut result = Vec::new();
        while let Some(idx) = rest.rfind('.') {
            result.push(&rest[idx..]);
            rest = &rest[..idx];
        }
        result.reverse();
        result
    }

    /// Returns the path components as a list of strings.
    ///
    /// Absolute paths start with "/" as the first component; `.` has no parts.
    #[must_use]
    pub fn parts(&self) -> Vec<&str> {
        if self.path.is_empty() || self.path == "." {
            return Vec::new();
        }

        let mut parts = Vec::new();
        if self.path.starts_with('/') {
            parts.push("/");
            let rest = &self.path[1..];
            if !rest.is_empty() {
                parts.extend(rest.split('/').filter(|s| !s.is_empty()));
            }
        } else {
            parts.extend(self.path.split('/').filter(|s| !s.is_empty()));
        }
        parts
    }

    /// Returns `true` if the path is absolute (starts with `/`).
    #[must_use]
    pub fn is_absolute(&self) -> bool {
        self.path.starts_with('/')
    }

    /// Returns the drive component of the path.
    ///
    /// For POSIX paths, this is always an empty string.
    #[must_use]
    #[expect(clippy::unused_self, reason = "mirrors Path method signature")]
    pub fn drive(&self) -> &'static str {
        ""
    }

    /// Returns the root component of the path.
    ///
    /// Returns "/" for absolute paths, and "" for relative paths.
    #[must_use]
    pub fn root(&self) -> &str {
        if self.is_absolute() { "/" } else { "" }
    }

    /// Returns the anchor component (drive + root).
    ///
    /// For POSIX paths, this is equal to the root ("/" or "").
    #[must_use]
    pub fn anchor(&self) -> &str {
        self.root()
    }

    /// Checks if the path is reserved (Windows DOS device names).
    ///
    /// For POSIX paths, this always returns `false`.
    #[must_use]
    #[expect(clippy::unused_self, reason = "mirrors Path method signature")]
    pub fn is_reserved(&self) -> bool {
        false
    }

    /// Returns `true` if this path matches the given glob pattern.
    ///
    /// # Errors
    /// Returns `ValueError` if the pattern has no parts (e.g. `""` or `"."`),
    /// or a resource error if execution-time limits are exceeded.
    pub fn match_pattern(&self, pattern: &str, case_sensitive: bool, tracker: &ResourceTracker) -> RunResult<bool> {
        let pattern_path = Self::new(pattern.to_owned());
        let pattern_parts = pattern_path.parts();
        if pattern_parts.is_empty() {
            return Err(SimpleException::new_msg(ExcType::ValueError, "empty pattern").into());
        }

        let path_parts = self.parts();
        if path_parts.len() < pattern_parts.len() {
            return Ok(false);
        }
        if path_parts.len() > pattern_parts.len() && !pattern_path.anchor().is_empty() {
            return Ok(false);
        }

        for (path_part, pattern_part) in path_parts.iter().rev().zip(pattern_parts.iter().rev()) {
            if *path_part == "/" || *pattern_part == "/" {
                if path_part != pattern_part {
                    return Ok(false);
                }
            } else if !SegmentPattern::new(pattern_part, case_sensitive)
                .matches_with_tracker(path_part, Some(tracker))?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Joins this path with another path component.
    ///
    /// If `other` is an absolute path, it replaces `self` entirely.
    #[must_use]
    pub fn joinpath(&self, other: &str) -> String {
        if other.starts_with('/') || self.path.is_empty() || self.path == "." {
            normalize_path(other.to_owned())
        } else if self.path.ends_with('/') {
            normalize_path(format!("{}{}", self.path, other))
        } else {
            normalize_path(format!("{}/{}", self.path, other))
        }
    }

    /// Returns a new path with the name changed.
    ///
    /// # Errors
    /// Returns CPython's `ValueError` message when `name` is empty, `.` or contains
    /// a separator (checked first, as CPython does), or when the path has no name.
    pub fn with_name(&self, name: &str) -> Result<String, String> {
        if name.is_empty() || name == "." || name.contains('/') {
            Err(format!("Invalid name {}", StringRepr(name)))
        } else if self.name().is_empty() {
            Err(self.empty_name_error())
        } else if let Some(parent) = self.parent() {
            if parent == "/" {
                Ok(format!("/{name}"))
            } else if parent == "." {
                // Relative path without directory - just use the new name
                Ok(name.to_owned())
            } else {
                Ok(format!("{parent}/{name}"))
            }
        } else {
            Ok(name.to_owned())
        }
    }

    /// Returns a new path with the stem changed (keeps the suffix).
    ///
    /// Mirrors CPython: with no suffix this is `with_name(stem)`, and an empty
    /// stem is only rejected separately when a suffix would be left on its own.
    pub fn with_stem(&self, stem: &str) -> Result<String, String> {
        let suffix = self.suffix();
        if suffix.is_empty() {
            self.with_name(stem)
        } else if stem.is_empty() {
            Err(format!("{} has a non-empty suffix", self.repr()))
        } else {
            self.with_name(&format!("{stem}{suffix}"))
        }
    }

    /// Returns a new path with the suffix changed.
    ///
    /// An empty `suffix` removes the existing one; any other must start with `.`
    /// (CPython rejects `'txt'` rather than adding the dot).
    pub fn with_suffix(&self, suffix: &str) -> Result<String, String> {
        let stem = self.stem();
        if stem.is_empty() {
            Err(self.empty_name_error())
        } else if !suffix.is_empty() && !suffix.starts_with('.') {
            Err(format!("Invalid suffix {}", StringRepr(suffix)))
        } else {
            self.with_name(&format!("{stem}{suffix}"))
        }
    }

    /// CPython's `ValueError` message for `with_*` on a path with no name (`.`, `/`).
    fn empty_name_error(&self) -> String {
        format!("{} has an empty name", self.repr())
    }

    /// The path's Python repr, `PosixPath('...')`, as used in `ValueError` messages.
    fn repr(&self) -> String {
        format!("PosixPath({})", StringRepr(&self.path))
    }

    /// Returns the path as a POSIX string (forward slashes).
    ///
    /// Since paths are already stored in POSIX format, this just returns the path.
    #[must_use]
    pub fn as_posix(&self) -> &str {
        &self.path
    }

    /// Creates a `Path` from the `Path()` constructor call.
    ///
    /// Accepts zero or more path segments that are joined together.
    /// - `Path()` returns `Path('.')`
    /// - `Path('a')` returns `Path('a')`
    /// - `Path('a', 'b', 'c')` returns `Path('a/b/c')`
    /// - If an absolute path appears, it replaces everything before it.
    pub fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let pos_args = args.into_pos_only("Path", vm.heap)?;
        defer_drop!(pos_args, vm);

        let path = match pos_args.as_slice() {
            [] => {
                // No arguments, return Path('.')
                Self::new(".".to_owned())
            }
            [single] => {
                // Single argument, just convert to Path
                Self::new(extract_path_string(single, vm)?.to_owned())
            }
            [first_arg, rest @ ..] => {
                let base = Self::new(extract_path_string(first_arg, vm)?.to_owned());
                fold_joinpath(base, rest, vm)?
            }
        };
        Ok(Value::Ref(vm.heap.allocate(HeapData::Path(path))))
    }
}

/// Splits a final component into `(stem, suffix)` as `posixpath.splitext` does.
///
/// The last `.` starts the suffix only if a non-dot character precedes it, so
/// `.bashrc` and `..` have no suffix while `a.` has the suffix `.`.
fn split_suffix(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(idx) if name[..idx].bytes().any(|b| b != b'.') => name.split_at(idx),
        _ => (name, ""),
    }
}

/// Classmethod `Path.cwd()`, also callable through instances: returns the VM's
/// virtual working directory without a host round-trip.
///
/// A pure-Python classmethod in CPython, so its arity errors count `cls`:
/// one stray positional is `takes 1 positional argument but 2 were given`.
pub(crate) fn class_cwd(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    if !matches!(args, ArgValues::Empty) {
        let (pos, kwargs) = args.into_parts();
        let given = pos.len();
        let first_kwarg = kwargs.first_key(vm.heap, vm.interns);
        pos.drop_with(vm);
        kwargs.drop_with(vm);
        // CPython reports a stray keyword before the positional count.
        return Err(match first_kwarg? {
            Some(key) => ExcType::type_error_unexpected_keyword("Path.cwd", &key),
            None => ExcType::type_error(format!(
                "Path.cwd() takes 1 positional argument but {} were given",
                given + 1
            )),
        });
    }
    let path = Path::new(vm.env.cwd.to_string());
    Ok(Value::Ref(vm.heap.allocate(HeapData::Path(path))))
}

/// `Path.glob(pattern, *, case_sensitive=None, recurse_symlinks=False)`.
#[derive(FromArgs)]
#[from_args(name = "Path.glob", style = def)]
struct PathGlobArgs {
    #[from_args(static_string = "PatternAttr")]
    pattern: Value,
    #[from_args(kw_only, default = Value::None)]
    case_sensitive: Value,
    #[from_args(kw_only, default = LaxBool::new(false))]
    recurse_symlinks: LaxBool,
}

/// `Path.rglob(pattern, *, case_sensitive=None, recurse_symlinks=False)`.
#[derive(FromArgs)]
#[from_args(name = "Path.rglob", style = def)]
struct PathRglobArgs {
    #[from_args(static_string = "PatternAttr")]
    pattern: Value,
    #[from_args(kw_only, default = Value::None)]
    case_sensitive: Value,
    #[from_args(kw_only, default = LaxBool::new(false))]
    recurse_symlinks: LaxBool,
}

/// `Path.walk(top_down=True, on_error=None, follow_symlinks=False)`.
#[derive(FromArgs)]
#[from_args(name = "Path.walk", style = def)]
struct PathWalkArgs {
    #[from_args(default = LaxBool::new(true))]
    top_down: LaxBool,
    #[from_args(default = Value::None)]
    on_error: Value,
    #[from_args(default = LaxBool::new(false))]
    follow_symlinks: LaxBool,
}

/// `Path.match(path_pattern, *, case_sensitive=None)`.
#[derive(FromArgs)]
#[from_args(name = "Path.match", style = def)]
struct PathMatchArgs {
    #[from_args(static_string = "PathPattern")]
    path_pattern: Value,
    #[from_args(kw_only, default = Value::None)]
    case_sensitive: Value,
}

/// `Path.glob()` / `Path.rglob()` on the receiver `base`: validates the
/// pattern as `pathlib` does, then hands it to [`glob_call`].
///
/// `rglob` is `glob` of `'**/' + pattern`. Any non-`None` `case_sensitive` is
/// truth-tested, and makes CPython list even literal pattern parts.
fn path_glob(
    base: &str,
    pattern: Value,
    recursive: bool,
    case_sensitive: Value,
    recurse_symlinks: LaxBool,
    vm: &mut VM<'_>,
) -> RunResult<CallResult> {
    defer_drop!(pattern, vm);
    defer_drop!(case_sensitive, vm);
    let pattern_text = match value_as_path_str(pattern, vm.heap, vm.interns) {
        Some(text) => text.to_owned(),
        // CPython's `bytes` handling trips over its own `str` separator.
        None if pattern.py_type(vm) == Type::Bytes => {
            return Err(ExcType::type_error("a bytes-like object is required, not 'str'"));
        }
        None => {
            return Err(ExcType::type_error(format!(
                "_path_splitroot_ex: path should be string, bytes or os.PathLike, not {}",
                pattern.py_type_name(vm)
            )));
        }
    };
    let case_sensitive = match case_sensitive {
        Value::None => None,
        value => Some(value.py_bool(vm)?),
    };
    let pattern_text = if recursive && !pattern_text.starts_with('/') {
        format!("**/{pattern_text}")
    } else {
        pattern_text
    };
    glob_call(base, &pattern_text, case_sensitive, recurse_symlinks.bool())
}

/// Extracts a string from a Value for use as a path.
fn extract_path_string<'a>(val: &'a Value, vm: &'a VM<'_>) -> RunResult<&'a str> {
    value_as_path_str(val, vm.heap, vm.interns)
        .ok_or_else(|| ExcType::type_error(format!("expected str or Path, got {}", val.py_type_name(vm))))
}

fn fold_joinpath(mut path: Path, parts: &[Value], vm: &VM<'_>) -> RunResult<Path> {
    for part in parts {
        path = Path::new(path.joinpath(extract_path_string(part, vm)?));
    }
    Ok(path)
}

/// Joins a `Path` with a path-like right operand for `Path.__truediv__`.
fn path_div(path: &Path, other: &Value, heap: &Heap, interns: &Interns) -> Option<Value> {
    let other = value_as_path_str(other, heap, interns)?;
    let result = path.joinpath(other);
    Some(Value::Ref(heap.allocate(HeapData::Path(Path::new(result)))))
}

/// Prepends a path-like left operand for `Path.__rtruediv__`.
fn path_rdiv(path: &Path, other: &Value, heap: &Heap, interns: &Interns) -> Option<Value> {
    let base = value_as_path_str(other, heap, interns)?;
    let result = Path::new(base.to_owned()).joinpath(path.as_str());
    Some(Value::Ref(heap.allocate(HeapData::Path(Path::new(result)))))
}

/// Normalizes a path string to POSIX format, matching CPython's `pathlib.PurePosixPath`.
///
/// - Converts backslashes to forward slashes
/// - Removes `.` components (e.g. `/a/./b` → `/a/b`)
/// - Collapses consecutive slashes (e.g. `//a///b` → `/a/b`)
/// - Removes trailing slashes (except for root "/")
/// - Turns a path with no components (`''`, `'.'`, `'./'`) into `.`
/// - Does NOT resolve `..` components (that requires I/O for symlinks)
fn normalize_path(mut path: String) -> String {
    // Convert backslashes to forward slashes
    if path.contains('\\') {
        path = path.replace('\\', "/");
    }

    // Fast path: non-empty, no `.` component and no consecutive or trailing slashes
    if !path.is_empty() && !path.contains("/.") && !path.contains("//") && !path.starts_with("./") {
        // Still strip trailing slashes
        while path.len() > 1 && path.ends_with('/') {
            path.pop();
        }
        return path;
    }

    let is_absolute = path.starts_with('/');
    let mut components: Vec<&str> = Vec::new();

    for part in path.split('/') {
        match part {
            "" | "." => {} // skip empty segments (from consecutive/trailing slashes) and "."
            other => components.push(other),
        }
    }

    if components.is_empty() {
        return if is_absolute { "/".to_owned() } else { ".".to_owned() };
    }

    let mut result = String::with_capacity(path.len());
    if is_absolute {
        result.push('/');
    }
    for (i, comp) in components.iter().enumerate() {
        if i > 0 {
            result.push('/');
        }
        result.push_str(comp);
    }
    result
}

impl Path {
    /// Resolves a known attribute by its `StaticStrings` variant.
    ///
    /// Returns `Some(value)` for recognized property names (`name`, `parent`,
    /// `stem`, `suffix`, `suffixes`, `parts`), or `None` if the variant doesn't
    /// correspond to a Path attribute. Used by `py_getattr` to share logic between
    /// the interned fast path and the heap string slow path.
    pub(crate) fn getattr_by_static(&self, ss: StaticStrings, heap: &Heap) -> Option<Value> {
        let v = match ss {
            StaticStrings::Name => allocate_string(self.name(), heap),
            StaticStrings::Parent => {
                if let Some(parent) = self.parent() {
                    let parent_path = Self::new(parent.to_owned());
                    Value::Ref(heap.allocate(HeapData::Path(parent_path)))
                } else {
                    // Return self when there's no parent (root or relative path)
                    let same_path = Self::new(self.as_str().to_owned());
                    Value::Ref(heap.allocate(HeapData::Path(same_path)))
                }
            }
            StaticStrings::Stem => allocate_string(self.stem(), heap),
            StaticStrings::Suffix => allocate_string(self.suffix(), heap),
            StaticStrings::Suffixes => {
                let suffixes = self.suffixes();
                let mut items = Vec::with_capacity(suffixes.len());
                for suffix in suffixes {
                    items.push(allocate_string(suffix, heap));
                }
                Value::Ref(heap.allocate(HeapData::List(List::new(items))))
            }
            StaticStrings::Parts => {
                let parts = self.parts();
                let mut items = SmallVec::with_capacity(parts.len());
                for part in parts {
                    items.push(allocate_string(part, heap));
                }
                allocate_tuple(items, heap)
            }
            StaticStrings::Anchor => allocate_string(self.anchor(), heap),
            StaticStrings::Drive => allocate_string(self.drive(), heap),
            StaticStrings::Root => allocate_string(self.root(), heap),
            _ => return None,
        };
        Some(v)
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, Path> {
    fn py_type(&self, _vm: &VM<'h>) -> Type {
        Type::Path
    }

    fn py_len(&self, _vm: &VM<'h>) -> Option<usize> {
        // Paths don't have a length in Python
        None
    }

    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let Some(HeapReadOutput::Path(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        Ok(Some(self.get(vm.heap).path == other.get(vm.heap).path))
    }

    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        let p = self.get(vm.heap);
        if let Some(cached) = p.cached_hash.get() {
            return Ok(Some(cached));
        }
        let mut hasher = DefaultHasher::new();
        p.as_str().hash(&mut hasher);
        let hash = HashValue::new(hasher.finish());
        p.cached_hash.set(Some(hash));
        Ok(Some(hash))
    }

    fn py_bool(&self, _vm: &mut VM<'h>) -> RunResult<bool> {
        // Paths are always truthy (even empty paths)
        Ok(true)
    }

    fn py_truediv_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(path_div(self.get(vm.heap), other, vm.heap, vm.interns))
    }

    fn py_rtruediv_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        Ok(path_rdiv(self.get(vm.heap), other, vm.heap, vm.interns))
    }

    fn py_str(&self, vm: &mut VM<'h>) -> RunResult<Value> {
        Ok(allocate_string(self.get(vm.heap).as_str(), vm.heap))
    }

    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, _heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        Ok(f.write_str(&self.get(vm.heap).repr())?)
    }

    /// Handles attribute calls on Path objects, including both pure methods (no I/O)
    /// and OS methods that require host system access.
    ///
    /// OS methods yield calls to the host; `iterdir` retains the receiver's spelling
    /// so the resume path can restore it on each returned entry.
    /// Pure methods (is_absolute, joinpath, etc.) are handled directly.
    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        let Some(method) = attr.static_string(vm.interns) else {
            args.drop_with(vm);
            return Err(ExcType::attribute_error(Type::Path, attr.as_str(vm.interns)));
        };

        if is_path_os_method(method) {
            let path = MontyPath::new(self.get(vm.heap).as_str().to_owned());
            return build_path_os_call(method, path, args, vm);
        }

        // Pure methods (no I/O)
        let value = match method {
            StaticStrings::Cwd => class_cwd(vm, args),
            StaticStrings::Glob => {
                let PathGlobArgs {
                    pattern,
                    case_sensitive,
                    recurse_symlinks,
                } = PathGlobArgs::from_args(args, vm)?;
                let base = self.get(vm.heap).as_str().to_owned();
                return path_glob(&base, pattern, false, case_sensitive, recurse_symlinks, vm);
            }
            StaticStrings::Rglob => {
                let PathRglobArgs {
                    pattern,
                    case_sensitive,
                    recurse_symlinks,
                } = PathRglobArgs::from_args(args, vm)?;
                let base = self.get(vm.heap).as_str().to_owned();
                return path_glob(&base, pattern, true, case_sensitive, recurse_symlinks, vm);
            }
            StaticStrings::Walk => {
                let PathWalkArgs {
                    top_down,
                    on_error,
                    follow_symlinks,
                } = PathWalkArgs::from_args(args, vm)?;
                let top = self.get(vm.heap).as_str().to_owned();
                return walk_call(top, true, top_down.bool(), on_error, follow_symlinks.bool(), vm);
            }
            StaticStrings::IsAbsolute => {
                args.check_zero_args("is_absolute", vm.heap)?;
                Ok(Value::Bool(self.get(vm.heap).is_absolute()))
            }
            StaticStrings::IsReserved => {
                args.check_zero_args("is_reserved", vm.heap)?;
                Ok(Value::Bool(self.get(vm.heap).is_reserved()))
            }
            StaticStrings::Match => {
                let PathMatchArgs {
                    path_pattern,
                    case_sensitive,
                } = PathMatchArgs::from_args(args, vm)?;
                defer_drop!(path_pattern, vm);
                defer_drop!(case_sensitive, vm);
                let case_sensitive = match case_sensitive {
                    Value::None => true,
                    value => value.py_bool(vm)?,
                };
                let Some(pattern_text) = value_as_path_str(path_pattern, vm.heap, vm.interns) else {
                    return Err(ExcType::type_error(format!(
                        "argument should be a str or an os.PathLike object where __fspath__ returns a str, not '{}'",
                        path_pattern.py_type_name(vm)
                    )));
                };
                let matched = self
                    .get(vm.heap)
                    .match_pattern(pattern_text, case_sensitive, &vm.heap.tracker)?;
                Ok(Value::Bool(matched))
            }
            StaticStrings::Joinpath => {
                let pos_args = args.into_pos_only("joinpath", vm.heap)?;
                defer_drop!(pos_args, vm);
                let path = fold_joinpath(self.get(vm.heap).clone(), pos_args.as_slice(), vm)?;
                Ok(Value::Ref(vm.heap.allocate(HeapData::Path(path))))
            }
            StaticStrings::WithName => {
                let name_val = args.get_one_arg("with_name", vm.heap)?;
                defer_drop!(name_val, vm);
                let name = name_val.try_to_str(vm)?.to_owned();
                let result = self
                    .get(vm.heap)
                    .with_name(&name)
                    .map_err(|e| SimpleException::new_msg(ExcType::ValueError, &e))?;
                Ok(Value::Ref(vm.heap.allocate(HeapData::Path(Path::new(result)))))
            }
            StaticStrings::WithStem => {
                let stem_val = args.get_one_arg("with_stem", vm.heap)?;
                defer_drop!(stem_val, vm);
                let stem = stem_val.try_to_str(vm)?.to_owned();
                let result = self
                    .get(vm.heap)
                    .with_stem(&stem)
                    .map_err(|e| SimpleException::new_msg(ExcType::ValueError, &e))?;
                Ok(Value::Ref(vm.heap.allocate(HeapData::Path(Path::new(result)))))
            }
            StaticStrings::WithSuffix => {
                let suffix_val = args.get_one_arg("with_suffix", vm.heap)?;
                defer_drop!(suffix_val, vm);
                let suffix = suffix_val.try_to_str(vm)?.to_owned();
                let result = self
                    .get(vm.heap)
                    .with_suffix(&suffix)
                    .map_err(|e| SimpleException::new_msg(ExcType::ValueError, &e))?;
                Ok(Value::Ref(vm.heap.allocate(HeapData::Path(Path::new(result)))))
            }
            StaticStrings::AsPosix | StaticStrings::Fspath => {
                args.check_zero_args(method.into(), vm.heap)?;
                Ok(allocate_string(self.get(vm.heap).as_posix(), vm.heap))
            }
            StaticStrings::Open => {
                // `Path.open(mode='r', ...)` is `open(self, mode, ...)` with
                // `self` prepended as the implicit `file` argument. Reuses
                // `builtin_open`'s mode/kwarg validation (including rejection
                // of `+`/`x` modes and non-default `buffering`/`encoding`/
                // `errors`/`newline`) so the two entry points stay in sync.
                //
                // The `inc_ref` is required because the prepended `Value::Ref`
                // is dropped by `builtin_open` via `defer_drop!` once the path
                // string has been extracted, balancing the refcount.
                let args = args.prepend(self.clone_value(vm.heap));
                return builtin_open(vm, args);
            }
            _ => {
                args.drop_with(vm);
                return Err(ExcType::attribute_error(Type::Path, attr.as_str(vm.interns)));
            }
        };
        value.map(CallResult::Value)
    }

    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        // Fast path: interned strings can be matched by ID without string comparison
        if let Some(ss) = attr.static_string(vm.interns) {
            if let Some(v) = self.get(vm.heap).getattr_by_static(ss, vm.heap) {
                return Ok(Some(CallResult::Value(v)));
            }
            return Err(ExcType::attribute_error(Type::Path, attr.as_str(vm.interns)));
        }
        // Slow path: heap-allocated strings need string comparison
        let attr_str = attr.as_str(vm.interns);
        let ss = match attr_str {
            "anchor" => StaticStrings::Anchor,
            "drive" => StaticStrings::Drive,
            "name" => StaticStrings::Name,
            "parent" => StaticStrings::Parent,
            "parts" => StaticStrings::Parts,
            "root" => StaticStrings::Root,
            "stem" => StaticStrings::Stem,
            "suffix" => StaticStrings::Suffix,
            "suffixes" => StaticStrings::Suffixes,
            _ => return Err(ExcType::attribute_error(Type::Path, attr_str)),
        };
        let v = self
            .get(vm.heap)
            .getattr_by_static(ss, vm.heap)
            .expect("matched attribute must produce a value");
        Ok(Some(CallResult::Value(v)))
    }
}

impl HeapItem for Path {
    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // Path doesn't contain heap references, nothing to do
    }
}
