# Pseudo Filesystem Access via pathlib Module

## Overview

Implement sandboxed filesystem access through Python's `pathlib` module. The design uses:
- **OsAccess trait**: Trait with methods like `stat()`, `exists()`, `read_bytes()` - passed as `impl OsAccess` generic
- **External function mechanism**: Path filesystem methods yield via existing external function pattern
- **Run loop interception**: The run loop intercepts path-related external calls and routes them through the `OsAccess` implementation

## Architecture

### Key Types

```rust
/// Result of stat() operation - matches Python's os.stat_result
///
/// Contains standard POSIX stat fields. All fields are required for full compatibility,
/// but implementations may return 0 for fields they don't support.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Stat {
    /// File mode (permissions + file type bits)
    pub st_mode: u32,
    /// Inode number
    pub st_ino: u64,
    /// Device identifier
    pub st_dev: u64,
    /// Number of hard links
    pub st_nlink: u64,
    /// User ID of owner
    pub st_uid: u32,
    /// Group ID of owner
    pub st_gid: u32,
    /// File size in bytes
    pub st_size: u64,
    /// Time of most recent access (Unix timestamp)
    pub st_atime: f64,
    /// Time of most recent content modification (Unix timestamp)
    pub st_mtime: f64,
    /// Time of most recent metadata change (Unix timestamp)
    pub st_ctime: f64,
}

impl Stat {
    // File type checks (from st_mode, matching Python's stat module)
    // See: https://github.com/python/cpython/blob/3.14/Lib/stat.py

    /// S_ISDIR - is a directory
    pub fn is_dir(&self) -> bool { self.st_mode & 0o170000 == 0o040000 }
    /// S_ISREG - is a regular file
    pub fn is_file(&self) -> bool { self.st_mode & 0o170000 == 0o100000 }
    /// S_ISLNK - is a symbolic link
    pub fn is_symlink(&self) -> bool { self.st_mode & 0o170000 == 0o120000 }
    /// S_ISBLK - is a block device
    pub fn is_block_device(&self) -> bool { self.st_mode & 0o170000 == 0o060000 }
    /// S_ISCHR - is a character device
    pub fn is_char_device(&self) -> bool { self.st_mode & 0o170000 == 0o020000 }
    /// S_ISFIFO - is a FIFO (named pipe)
    pub fn is_fifo(&self) -> bool { self.st_mode & 0o170000 == 0o010000 }
    /// S_ISSOCK - is a socket
    pub fn is_socket(&self) -> bool { self.st_mode & 0o170000 == 0o140000 }
}
```

/// Trait for filesystem access - used as `impl OsAccess` generic parameter
pub trait OsAccess: fmt::Debug {
    fn stat(&self, path: &str) -> Result<Stat, String>;
    fn exists(&self, path: &str) -> Result<bool, String>;
    fn is_file(&self, path: &str) -> Result<bool, String>;
    fn is_dir(&self, path: &str) -> Result<bool, String>;
    fn is_symlink(&self, path: &str) -> Result<bool, String>;
    fn read_bytes(&self, path: &str) -> Result<Vec<u8>, String>;
    fn read_text(&self, path: &str, encoding: &str) -> Result<String, String>;
    fn iterdir(&self, path: &str) -> Result<Vec<String>, String>;
    fn resolve(&self, path: &str) -> Result<String, String>;
    fn absolute(&self, path: &str) -> Result<String, String>;
}
```

### Execution Flow

```
Python: Path('/foo').exists()
    ↓
VM: yields External(__os_access_exists__, ["/foo"])
    ↓
run loop: intercepts __os_access_* calls
    ↓
run loop: calls os_access.exists("/foo")
    ↓
run loop: feeds result back to VM
    ↓
Python: receives True/False
```

### API

```rust
impl MontyRun {
    pub fn run<R: ResourceTracker, O: OsAccess>(
        &self,
        inputs: impl Into<Inputs>,
        tracker: R,
        print: &mut impl PrintWriter,
        os_access: Option<&O>,  // NEW - generic impl OsAccess
    ) -> Result<MontyObject, MontyException>;
}
```

Note: Using `impl OsAccess` (via generic `O: OsAccess`) allows monomorphization and avoids dynamic dispatch overhead. The `Option` allows running without filesystem access.

## Implementation Plan

### Phase 1: Core Types (`crates/monty/src/os_access.rs`) - NEW FILE

```rust
use std::fmt;

/// Result of stat() operation - matches Python's os.stat_result
///
/// Contains standard POSIX stat fields. All fields are required for full compatibility,
/// but implementations may return 0 for fields they don't support.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Stat {
    /// File mode (permissions + file type bits)
    pub st_mode: u32,
    /// Inode number
    pub st_ino: u64,
    /// Device identifier
    pub st_dev: u64,
    /// Number of hard links
    pub st_nlink: u64,
    /// User ID of owner
    pub st_uid: u32,
    /// Group ID of owner
    pub st_gid: u32,
    /// File size in bytes
    pub st_size: u64,
    /// Time of most recent access (Unix timestamp)
    pub st_atime: f64,
    /// Time of most recent content modification (Unix timestamp)
    pub st_mtime: f64,
    /// Time of most recent metadata change (Unix timestamp)
    pub st_ctime: f64,
}

impl Stat {
    // File type checks (from st_mode, matching Python's stat module)
    // See: https://github.com/python/cpython/blob/3.14/Lib/stat.py

    /// S_ISDIR - is a directory
    pub fn is_dir(&self) -> bool { self.st_mode & 0o170000 == 0o040000 }
    /// S_ISREG - is a regular file
    pub fn is_file(&self) -> bool { self.st_mode & 0o170000 == 0o100000 }
    /// S_ISLNK - is a symbolic link
    pub fn is_symlink(&self) -> bool { self.st_mode & 0o170000 == 0o120000 }
    /// S_ISBLK - is a block device
    pub fn is_block_device(&self) -> bool { self.st_mode & 0o170000 == 0o060000 }
    /// S_ISCHR - is a character device
    pub fn is_char_device(&self) -> bool { self.st_mode & 0o170000 == 0o020000 }
    /// S_ISFIFO - is a FIFO (named pipe)
    pub fn is_fifo(&self) -> bool { self.st_mode & 0o170000 == 0o010000 }
    /// S_ISSOCK - is a socket
    pub fn is_socket(&self) -> bool { self.st_mode & 0o170000 == 0o140000 }
}

/// Trait for sandboxed filesystem access.
///
/// Implementations provide the actual filesystem operations. The runtime
/// calls these methods when Python code uses pathlib.Path filesystem methods.
/// Used as a generic parameter (`impl OsAccess`) to avoid dynamic dispatch.
pub trait OsAccess: fmt::Debug {
    fn stat(&self, path: &str) -> Result<Stat, String>;
    fn exists(&self, path: &str) -> Result<bool, String>;
    fn is_file(&self, path: &str) -> Result<bool, String>;
    fn is_dir(&self, path: &str) -> Result<bool, String>;
    fn is_symlink(&self, path: &str) -> Result<bool, String>;
    fn read_bytes(&self, path: &str) -> Result<Vec<u8>, String>;
    fn read_text(&self, path: &str, encoding: &str) -> Result<String, String>;
    fn iterdir(&self, path: &str) -> Result<Vec<String>, String>;
    fn resolve(&self, path: &str) -> Result<String, String>;
    fn absolute(&self, path: &str) -> Result<String, String>;
}
```

### Phase 2: Path Type (`crates/monty/src/types/path.rs`) - NEW FILE

Internal heap-allocated Path type with pure methods:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Path {
    path: String,
}

impl Path {
    pub fn new(path: String) -> Self { Self { path: normalize_path(path) } }
    pub fn as_str(&self) -> &str { &self.path }

    // Pure methods (no I/O)
    pub fn name(&self) -> &str { /* last component */ }
    pub fn parent(&self) -> Option<&str> { /* parent path */ }
    pub fn stem(&self) -> Option<&str> { /* name without suffix */ }
    pub fn suffix(&self) -> Option<&str> { /* file extension */ }
    pub fn parts(&self) -> Vec<&str> { /* path components */ }
    pub fn is_absolute(&self) -> bool { /* starts with / */ }
    pub fn joinpath(&self, other: &str) -> String { /* join paths */ }
    pub fn with_name(&self, name: &str) -> Result<String, String> { /* replace name */ }
    pub fn with_suffix(&self, suffix: &str) -> String { /* replace suffix */ }
}
```

### Phase 3: MontyPath in MontyObject (`crates/monty/src/object.rs`)

```rust
pub enum MontyObject {
    // ... existing variants ...

    /// A filesystem path from pathlib.Path
    Path(String),
}
```

### Phase 4: Reserved External Functions

Define reserved external function names for path operations:

| External Function | OsAccess Method | Returns |
|-------------------|-----------------|---------|
| `__os_access_stat__` | `stat(path)` | `Stat` → dict |
| `__os_access_exists__` | `exists(path)` | `bool` |
| `__os_access_is_file__` | `is_file(path)` | `bool` |
| `__os_access_is_dir__` | `is_dir(path)` | `bool` |
| `__os_access_is_symlink__` | `is_symlink(path)` | `bool` |
| `__os_access_read_bytes__` | `read_bytes(path)` | `bytes` |
| `__os_access_read_text__` | `read_text(path, encoding)` | `str` |
| `__os_access_iterdir__` | `iterdir(path)` | `list[str]` |
| `__os_access_resolve__` | `resolve(path)` | `str` |
| `__os_access_absolute__` | `absolute(path)` | `str` |

### Phase 5: Run Loop Integration (`crates/monty/src/run.rs`)

Modify the run loop to intercept `__os_access_*` external calls:

```rust
impl MontyRun {
    pub fn run<R: ResourceTracker, O: OsAccess>(
        &self,
        inputs: impl Into<Inputs>,
        tracker: R,
        print: &mut impl PrintWriter,
        os_access: Option<&O>,
    ) -> Result<MontyObject, MontyException> {
        // ... setup ...

        loop {
            match vm.run_module()? {
                FrameExit::Return(value) => return Ok(MontyObject::new(value, heap, interns)),
                FrameExit::ExternalCall(ext_id, args) => {
                    let func_name = interns.get_ext_function_name(ext_id);

                    // Intercept __os_access_* calls
                    if let Some(result) = handle_os_access_call(func_name, &args, os_access, heap)? {
                        vm.push(result);
                        continue;
                    }

                    // Regular external function - yield to caller
                    // ... existing logic ...
                }
            }
        }
    }
}

fn handle_os_access_call<O: OsAccess>(
    func_name: &str,
    args: &ArgValues,
    os_access: Option<&O>,
    heap: &mut Heap<impl ResourceTracker>,
) -> Result<Option<Value>, MontyException> {
    let os = os_access.ok_or_else(|| /* OSError: no filesystem access */)?;

    match func_name {
        "__os_access_exists__" => {
            let path = extract_path_arg(args)?;
            let result = os.exists(&path).map_err(|e| /* convert to OSError */)?;
            Ok(Some(Value::Bool(result)))
        }
        "__os_access_stat__" => {
            let path = extract_path_arg(args)?;
            let stat = os.stat(&path).map_err(|e| /* convert to OSError */)?;
            Ok(Some(stat_to_namedtuple(stat, heap)?))
        }
        // ... other __os_access_* handlers ...
        _ => Ok(None),  // Not an os_access call
    }
}
```

### Phase 6: Module and Type Registration

#### 6.1 StaticStrings (`crates/monty/src/intern.rs`)

```rust
// pathlib module
#[strum(serialize = "pathlib")]
Pathlib,
#[strum(serialize = "Path")]
PathClass,

// Path properties
#[strum(serialize = "name")]
Name,
#[strum(serialize = "parent")]
Parent,
// ... etc

// Reserved external functions
#[strum(serialize = "__os_access_stat__")]
OsAccessStat,
#[strum(serialize = "__os_access_exists__")]
OsAccessExists,
// ... etc
```

#### 6.2 BuiltinModule (`crates/monty/src/modules/mod.rs`)

```rust
pub(crate) enum BuiltinModule {
    Sys,
    Typing,
    Asyncio,
    Pathlib,  // NEW
}
```

#### 6.3 pathlib module (`crates/monty/src/modules/pathlib.rs`) - NEW FILE

```rust
pub fn create_module(
    heap: &mut Heap<impl ResourceTracker>,
    interns: &Interns,
) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Pathlib);
    module.set_attr(
        StaticStrings::PathClass,
        Value::Builtin(Builtins::Type(Type::Path)),
        heap,
        interns,
    );
    heap.allocate(HeapData::Module(module))
}
```

#### 6.4 HeapData and Type

- Add `HeapData::Path(Path)` to `heap.rs`
- Add `Type::Path` to `types/type.rs`

### Phase 7: Path Method Implementation

When a Path method like `exists()` is called:

1. `call_attr` in `call.rs` dispatches to Path's `py_call_attr`
2. For filesystem methods, Path emits an external function call
3. The run loop intercepts and routes through OsAccess

```rust
// In Path's py_call_attr implementation
impl PyTrait for Path {
    fn py_call_attr(&self, heap, attr, args, interns) -> RunResult<Value> {
        match attr.as_static_string() {
            // Pure methods - handle directly
            Some(StaticStrings::Name) => Ok(self.name().into()),
            Some(StaticStrings::Joinpath) => { /* ... */ }

            // Filesystem methods - need external call
            // These should trigger FrameExit::ExternalCall
            Some(StaticStrings::Exists) |
            Some(StaticStrings::IsFile) |
            Some(StaticStrings::Stat) => {
                // Return a marker that causes VM to yield external call
                // Implementation TBD - may need CallResult::OsAccess variant
            }
            _ => Err(ExcType::attribute_error(Type::Path, attr_name))
        }
    }
}
```

**Implementation detail**: We need a way for `py_call_attr` to signal that an external call is needed. Options:
1. Add `CallResult::OsAccess(OsAccessOp, args)` variant
2. Store external function IDs at compile time
3. Have Path methods directly construct `FrameExit::ExternalCall`

Recommend option 1 - cleanest separation.

## Files to Modify

| File | Change |
|------|--------|
| `crates/monty/src/os_access.rs` | **NEW** - Stat struct and OsAccess trait |
| `crates/monty/src/types/path.rs` | **NEW** - Path type with pure methods |
| `crates/monty/src/types/mod.rs` | Add `pub(crate) mod path;` |
| `crates/monty/src/heap.rs` | Add `HeapData::Path(Path)` variant |
| `crates/monty/src/types/type.rs` | Add `Type::Path` variant |
| `crates/monty/src/intern.rs` | Add StaticStrings for pathlib |
| `crates/monty/src/modules/mod.rs` | Add `BuiltinModule::Pathlib` |
| `crates/monty/src/modules/pathlib.rs` | **NEW** - Module creation |
| `crates/monty/src/object.rs` | Add `MontyObject::Path(String)` |
| `crates/monty/src/run.rs` | Add `os_access` parameter, intercept calls |
| `crates/monty/src/bytecode/vm/call.rs` | Handle Path method calls |
| `crates/monty/src/lib.rs` | Export `OsAccess`, `Stat` |

## Path Methods

### Pure Methods (implemented directly)
| Method | Description |
|--------|-------------|
| `name` | Final path component |
| `parent` | Parent directory path |
| `parts` | Tuple of path components |
| `stem` | Name without suffix |
| `suffix` | File extension |
| `suffixes` | List of extensions |
| `is_absolute()` | Check if absolute |
| `joinpath(*args)` | Join components |
| `with_name(name)` | Replace name |
| `with_suffix(suffix)` | Replace suffix |
| `as_posix()` | POSIX string |
| `__truediv__` | `/` operator |
| `__str__`, `__repr__` | String representations |

### Filesystem Methods (via OsAccess)
| Method | External Function |
|--------|-------------------|
| `exists()` | `__os_access_exists__` |
| `is_file()` | `__os_access_is_file__` |
| `is_dir()` | `__os_access_is_dir__` |
| `is_symlink()` | `__os_access_is_symlink__` |
| `stat()` | `__os_access_stat__` |
| `read_bytes()` | `__os_access_read_bytes__` |
| `read_text(encoding)` | `__os_access_read_text__` |
| `iterdir()` | `__os_access_iterdir__` |
| `resolve()` | `__os_access_resolve__` |
| `absolute()` | `__os_access_absolute__` |

## Testing

### Pure method tests (`crates/monty/test_cases/pathlib__pure.py`)
```python
from pathlib import Path

p = Path('/usr/local/bin/python')
assert p.name == 'python'
assert str(p.parent) == '/usr/local/bin'
assert p.stem == 'python'
assert p.is_absolute() == True
assert str(p / 'lib') == '/usr/local/bin/python/lib'
```

### Filesystem tests (`crates/monty-python/tests/test_pathlib.py`)
```python
def test_path_exists(monty_with_fs):
    """Test with mock OsAccess implementation."""
    result = monty_with_fs.run("from pathlib import Path; Path('/test').exists()")
    assert result == True
```

## Verification

1. `make test-cases` - pure method tests pass
2. `make test-py` - filesystem tests with mock OsAccess pass
3. `make lint-rs && make format-rs` - code quality
4. Manual test: playground script with real OsAccess implementation
