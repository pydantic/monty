//! Python `pathlib.Path` type implementation.
//!
//! Provides a path object with both pure methods (no I/O) and filesystem methods
//! (require `OsAccess` implementation). Pure methods are handled directly by the VM,
//! while filesystem methods yield external function calls for the host to resolve.

use std::fmt::Write;

use ahash::AHashSet;
use smallvec::SmallVec;

use crate::{
    args::{ArgValues, KwargsValues},
    defer_drop,
    exception_private::{ExcType, RunResult},
    heap::{DropWithHeap, Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings, StringId},
    os::OsFunction,
    resource::{DepthGuard, ResourceError, ResourceTracker},
    types::{AttrCallResult, PyTrait, Str, Type, allocate_tuple},
    value::{EitherStr, Value},
};

/// Python `pathlib.Path` object representing a filesystem path.
///
/// Stores a normalized POSIX path string. Windows-style paths are converted
/// to POSIX format (backslashes to forward slashes).
///
/// The path is immutable - all operations that would modify the path return
/// new `Path` objects or strings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct Path {
    /// The normalized path string.
    path: String,
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
        }
    }

    /// Returns the path as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.path
    }

    /// Returns the final component of the path.
    ///
    /// Returns an empty string if the path ends with a separator or is empty.
    #[must_use]
    pub fn name(&self) -> &str {
        self.path.rsplit_once('/').map_or(self.path.as_str(), |(_, name)| name)
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
        let name = self.name();
        if name.starts_with('.') && !name[1..].contains('.') {
            // Hidden file without extension (e.g., ".bashrc")
            return name;
        }
        name.rsplit_once('.').map_or(name, |(stem, _)| stem)
    }

    /// Returns the file extension (last suffix), including the leading dot.
    ///
    /// Returns an empty string if there is no extension.
    #[must_use]
    pub fn suffix(&self) -> &str {
        let name = self.name();
        if name.starts_with('.') && !name[1..].contains('.') {
            // Hidden file without extension (e.g., ".bashrc")
            return "";
        }
        name.rfind('.').map_or("", |idx| &name[idx..])
    }

    /// Returns all file extensions as a list of strings.
    ///
    /// Each suffix includes its leading dot. Returns an empty list if no extensions.
    #[must_use]
    pub fn suffixes(&self) -> Vec<&str> {
        let name = self.name();
        if name.is_empty() || name == "." || name == ".." {
            return Vec::new();
        }

        let start_idx = usize::from(name.starts_with('.'));
        let search_str = &name[start_idx..];

        let mut result = Vec::new();
        let mut pos = 0;
        while let Some(idx) = search_str[pos..].find('.') {
            let abs_idx = pos + idx;
            // Each suffix is from this dot to the end or next dot
            let suffix_end = search_str[abs_idx + 1..]
                .find('.')
                .map_or(search_str.len(), |next| abs_idx + 1 + next);
            result.push(&name[start_idx + abs_idx..start_idx + suffix_end]);
            pos = abs_idx + 1;
        }
        result
    }

    /// Returns the path components as a list of strings.
    ///
    /// Absolute paths start with "/" as the first component.
    #[must_use]
    pub fn parts(&self) -> Vec<&str> {
        if self.path.is_empty() {
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
    /// Returns an error if the path has no name or if the new name is empty.
    pub fn with_name(&self, name: &str) -> Result<String, String> {
        if name.is_empty() {
            return Err("Invalid name: empty string".to_owned());
        }
        if name.contains('/') {
            return Err(format!("Invalid name: {name:?} contains path separator"));
        }
        if self.name().is_empty() {
            return Err("Path has no name".to_owned());
        }

        if let Some(parent) = self.parent() {
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
    /// # Errors
    /// Returns an error if the path has no name or if the new stem is empty.
    pub fn with_stem(&self, stem: &str) -> Result<String, String> {
        if stem.is_empty() {
            return Err("Invalid stem: empty string".to_owned());
        }
        if stem.contains('/') {
            return Err(format!("Invalid stem: {stem:?} contains path separator"));
        }
        if self.name().is_empty() {
            return Err("Path has no name".to_owned());
        }

        let suffix = self.suffix();
        let new_name = format!("{stem}{suffix}");
        self.with_name(&new_name)
    }

    /// Returns a new path with the suffix changed.
    ///
    /// If the suffix is empty, removes the existing suffix.
    /// If the suffix doesn't start with '.', it's added.
    pub fn with_suffix(&self, suffix: &str) -> Result<String, String> {
        if self.name().is_empty() {
            return Err("Path has no name".to_owned());
        }

        let suffix = if suffix.is_empty() || suffix.starts_with('.') {
            suffix.to_owned()
        } else {
            format!(".{suffix}")
        };

        if suffix.contains('/') {
            return Err(format!("Invalid suffix: {suffix:?} contains path separator"));
        }

        let stem = self.stem();
        let new_name = format!("{stem}{suffix}");
        self.with_name(&new_name)
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
    pub fn init(heap: &mut Heap<impl ResourceTracker>, args: ArgValues, interns: &Interns) -> RunResult<Value> {
        let pos_args = args.into_pos_only("Path", heap)?;
        defer_drop!(pos_args, heap);

        let path = match pos_args.as_slice() {
            [] => {
                // No arguments, return Path('.')
                Self::new(".".to_owned())
            }
            [single] => {
                // Single argument, just convert to Path
                Self::new(extract_path_string(single, heap, interns)?.to_owned())
            }
            [first_arg, rest @ ..] => {
                let base = Self::new(extract_path_string(first_arg, heap, interns)?.to_owned());
                fold_joinpath(base, rest, heap, interns)?
            }
        };
        Ok(Value::Ref(heap.allocate(HeapData::Path(path))?))
    }
}

/// Extracts a string from a Value for use as a path.
fn extract_path_string<'a>(
    val: &Value,
    heap: &'a Heap<impl ResourceTracker>,
    interns: &'a Interns,
) -> RunResult<&'a str> {
    match val {
        Value::InternString(string_id) => Ok(interns.get_str(*string_id)),
        Value::Ref(heap_id) => match heap.get(*heap_id) {
            HeapData::Str(s) => Ok(s.as_str()),
            HeapData::Path(p) => Ok(p.as_str()),
            _ => Err(ExcType::type_error(format!(
                "expected str or Path, got {}",
                val.py_type(heap)
            ))),
        },
        _ => Err(ExcType::type_error(format!(
            "expected str or Path, got {}",
            val.py_type(heap)
        ))),
    }
}

fn fold_joinpath(
    mut path: Path,
    parts: &[Value],
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Path> {
    for part in parts {
        path = Path::new(path.joinpath(extract_path_string(part, heap, interns)?));
    }
    Ok(path)
}

/// Handles the `/` operator for Path objects (path concatenation).
///
/// In Python, `Path('/usr') / 'bin'` produces `Path('/usr/bin')`.
pub(crate) fn path_div(
    path_id: HeapId,
    other: &Value,
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> RunResult<Option<Value>> {
    // Extract the right-hand side as a string
    let other_str = match other {
        Value::InternString(string_id) => interns.get_str(*string_id).to_owned(),
        Value::Ref(other_id) => match heap.get(*other_id) {
            HeapData::Str(s) => s.as_str().to_owned(),
            HeapData::Path(p) => p.as_str().to_owned(),
            _ => return Ok(None),
        },
        _ => return Ok(None),
    };

    // Get the path string
    let path_str = match heap.get(path_id) {
        HeapData::Path(p) => p.as_str().to_owned(),
        _ => return Ok(None),
    };

    // Perform path concatenation
    let result = Path::new(path_str).joinpath(&other_str);
    Ok(Some(Value::Ref(heap.allocate(HeapData::Path(Path::new(result)))?)))
}

/// Normalizes a path string to POSIX format.
///
/// - Converts backslashes to forward slashes
/// - Removes trailing slashes (except for root "/")
/// - Does NOT resolve `.` or `..` components (that requires I/O for symlinks)
fn normalize_path(mut path: String) -> String {
    // Convert backslashes to forward slashes
    if path.contains('\\') {
        path = path.replace('\\', "/");
    }

    // Remove trailing slashes, but keep root "/"
    while path.len() > 1 && path.ends_with('/') {
        path.pop();
    }

    path
}

/// Prepends the path string argument to existing arguments for OS calls.
///
/// OS functions expect the path as the first argument, so we need to
/// combine it with any additional arguments passed to the method.
fn prepend_path_arg(path_arg: Value, args: ArgValues) -> ArgValues {
    match args {
        ArgValues::Empty => ArgValues::One(path_arg),
        ArgValues::One(v) => ArgValues::Two(path_arg, v),
        ArgValues::Two(a, b) => ArgValues::ArgsKargs {
            args: vec![path_arg, a, b],
            kwargs: KwargsValues::Empty,
        },
        ArgValues::Kwargs(kwargs) => ArgValues::ArgsKargs {
            args: vec![path_arg],
            kwargs,
        },
        ArgValues::ArgsKargs { args: mut vals, kwargs } => {
            vals.insert(0, path_arg);
            ArgValues::ArgsKargs { args: vals, kwargs }
        }
    }
}

impl PyTrait for Path {
    fn py_type(&self, _heap: &Heap<impl ResourceTracker>) -> Type {
        Type::Path
    }

    fn py_len(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> Option<usize> {
        // Paths don't have a length in Python
        None
    }

    fn py_eq(
        &self,
        other: &Self,
        _heap: &mut Heap<impl ResourceTracker>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> Result<bool, ResourceError> {
        Ok(self.path == other.path)
    }

    fn py_bool(&self, _heap: &Heap<impl ResourceTracker>, _interns: &Interns) -> bool {
        // Paths are always truthy (even empty paths)
        true
    }

    fn py_repr_fmt(
        &self,
        f: &mut impl Write,
        _heap: &Heap<impl ResourceTracker>,
        _heap_ids: &mut AHashSet<HeapId>,
        _guard: &mut DepthGuard,
        _interns: &Interns,
    ) -> std::fmt::Result {
        // Format like: PosixPath('/usr/bin')
        write!(f, "PosixPath('{}')", self.path)
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // Path doesn't contain heap references, nothing to do
    }

    fn py_estimate_size(&self) -> usize {
        std::mem::size_of::<Self>() + self.path.capacity()
    }

    fn py_call_attr(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<Value> {
        let Some(method) = attr.static_string() else {
            args.drop_with_heap(heap);
            return Err(ExcType::attribute_error(Type::Path, attr.as_str(interns)));
        };

        match method {
            // Pure methods (no I/O)
            StaticStrings::IsAbsolute => {
                args.check_zero_args("is_absolute", heap)?;
                Ok(Value::Bool(self.is_absolute()))
            }
            StaticStrings::Joinpath => {
                let pos_args = args.into_pos_only("joinpath", heap)?;
                defer_drop!(pos_args, heap);
                let path = fold_joinpath(self.clone(), pos_args.as_slice(), heap, interns)?;
                Ok(Value::Ref(heap.allocate(HeapData::Path(path))?))
            }
            StaticStrings::WithName => {
                let name_val = args.get_one_arg("with_name", heap)?;
                defer_drop!(name_val, heap);
                let name = extract_path_string(name_val, heap, interns)?;
                let result = self
                    .with_name(name)
                    .map_err(|e| crate::exception_private::SimpleException::new_msg(ExcType::ValueError, &e))?;
                Ok(Value::Ref(heap.allocate(HeapData::Path(Self::new(result)))?))
            }
            StaticStrings::WithStem => {
                let stem_val = args.get_one_arg("with_stem", heap)?;
                defer_drop!(stem_val, heap);
                let stem = extract_path_string(stem_val, heap, interns)?;
                let result = self
                    .with_stem(stem)
                    .map_err(|e| crate::exception_private::SimpleException::new_msg(ExcType::ValueError, &e))?;
                Ok(Value::Ref(heap.allocate(HeapData::Path(Self::new(result)))?))
            }
            StaticStrings::WithSuffix => {
                let suffix_val = args.get_one_arg("with_suffix", heap)?;
                defer_drop!(suffix_val, heap);
                let suffix = extract_path_string(suffix_val, heap, interns)?;
                let result = self
                    .with_suffix(suffix)
                    .map_err(|e| crate::exception_private::SimpleException::new_msg(ExcType::ValueError, &e))?;
                Ok(Value::Ref(heap.allocate(HeapData::Path(Self::new(result)))?))
            }
            StaticStrings::AsPosix | StaticStrings::Fspath => {
                args.check_zero_args(method.into(), heap)?;
                // Both as_posix() and __fspath__() return the string representation
                Ok(Value::Ref(
                    heap.allocate(HeapData::Str(Str::new(self.as_posix().to_owned())))?,
                ))
            }
            _ => {
                args.drop_with_heap(heap);
                Err(ExcType::attribute_error(Type::Path, attr.as_str(interns)))
            }
        }
    }

    fn py_call_attr_raw(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &EitherStr,
        args: ArgValues,
        interns: &Interns,
    ) -> RunResult<AttrCallResult> {
        let Some(method) = attr.static_string() else {
            return self.py_call_attr(heap, attr, args, interns).map(AttrCallResult::Value);
        };

        // Check if this is an OS method that requires host system access
        if let Ok(os_fn) = OsFunction::try_from(method) {
            // Package path as first argument for OS call (as Path, not string)
            let path_arg = Value::Ref(heap.allocate(HeapData::Path(self.clone()))?);
            let os_args = prepend_path_arg(path_arg, args);
            return Ok(AttrCallResult::OsCall(os_fn, os_args));
        }

        // Fall back to py_call_attr for pure methods
        self.py_call_attr(heap, attr, args, interns).map(AttrCallResult::Value)
    }

    fn py_getattr(
        &self,
        attr_id: StringId,
        heap: &mut Heap<impl ResourceTracker>,
        interns: &Interns,
    ) -> RunResult<Option<AttrCallResult>> {
        let v = match StaticStrings::from_string_id(attr_id) {
            // Properties returning strings
            Some(StaticStrings::Name) => {
                let name = self.name();
                Value::Ref(heap.allocate(HeapData::Str(Str::new(name.to_owned())))?)
            }
            Some(StaticStrings::Parent) => {
                if let Some(parent) = self.parent() {
                    let parent_path = Self::new(parent.to_owned());
                    Value::Ref(heap.allocate(HeapData::Path(parent_path))?)
                } else {
                    // Return self when there's no parent (root or relative path)
                    let same_path = Self::new(self.as_str().to_owned());
                    Value::Ref(heap.allocate(HeapData::Path(same_path))?)
                }
            }
            Some(StaticStrings::Stem) => {
                let stem = self.stem();
                Value::Ref(heap.allocate(HeapData::Str(Str::new(stem.to_owned())))?)
            }
            Some(StaticStrings::Suffix) => {
                let suffix = self.suffix();
                Value::Ref(heap.allocate(HeapData::Str(Str::new(suffix.to_owned())))?)
            }
            Some(StaticStrings::Suffixes) => {
                use crate::types::List;

                let suffixes = self.suffixes();
                let mut items = Vec::with_capacity(suffixes.len());
                for suffix in suffixes {
                    let str_id = heap.allocate(HeapData::Str(Str::new(suffix.to_owned())))?;
                    items.push(Value::Ref(str_id));
                }
                Value::Ref(heap.allocate(HeapData::List(List::new(items)))?)
            }
            Some(StaticStrings::Parts) => {
                let parts = self.parts();
                let mut items = SmallVec::with_capacity(parts.len());
                for part in parts {
                    let str_id = heap.allocate(HeapData::Str(Str::new(part.to_owned())))?;
                    items.push(Value::Ref(str_id));
                }
                allocate_tuple(items, heap)?
            }
            // Method is_absolute() returns bool - handled as property since it takes no args
            // NOTE: For method calls, we'd need to return a bound method. For now, properties only.
            _ => return Err(ExcType::attribute_error(Type::Path, interns.get_str(attr_id))),
        };
        Ok(Some(AttrCallResult::Value(v)))
    }
}
