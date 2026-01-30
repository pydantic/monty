# Plan: Path Filesystem Methods via External Functions

## Summary

Make Path filesystem methods work by yielding external calls with reserved `__os_access_*` function names. Uses the `py_call_attr_raw` mechanism that allows types to return `FrameExit` directly.

## Implementation Steps

### Step 1: Add `py_call_attr_raw` to PyTrait

**File:** `crates/monty/src/types/py_trait.rs`

Add a new method to `PyTrait` that returns `FrameExit` directly, enabling types to yield external calls:

```rust
/// Call an attribute method, returning a FrameExit directly.
///
/// This allows types to yield external calls (FrameExit::ExternalCall) or
/// other control flow. The default implementation calls py_call_attr and
/// wraps the result in FrameExit::Return.
///
/// Override this for types that need to yield external calls (like Path).
fn py_call_attr_raw(
    &mut self,
    heap: &mut Heap<impl ResourceTracker>,
    attr: &Attr,
    args: ArgValues,
    interns: &Interns,
) -> Result<FrameExit, RunError> {
    // Default: call py_call_attr and wrap in Return
    let value = self.py_call_attr(heap, attr, args, interns)?;
    Ok(FrameExit::Return(value))
}
```

Note: `FrameExit` needs to be accessible from `py_trait.rs`. May need to adjust imports or move the enum.

### Step 2: Update `call_attr` to Return `CallResult`

**File:** `crates/monty/src/bytecode/vm/call.rs`

Change `call_attr` signature and add special handling for Path:

```rust
fn call_attr(&mut self, obj: Value, name_id: StringId, args: ArgValues) -> Result<CallResult, RunError> {
    let attr = Attr::Interned(name_id);

    match obj {
        Value::Ref(heap_id) => {
            // Check for list.sort - needs special handling for key functions
            if name_id == StaticStrings::Sort && matches!(self.heap.get(heap_id), HeapData::List(_)) {
                let result = do_list_sort(heap_id, args, self.heap, self.interns, self.print_writer);
                obj.drop_with_heap(self.heap);
                return result.map(|()| CallResult::Push(Value::None));
            }

            // Check for Path - needs py_call_attr_raw for filesystem methods
            if matches!(self.heap.get(heap_id), HeapData::Path(_)) {
                return self.call_attr_path(heap_id, &attr, args, obj);
            }

            // Call the method on the heap object
            let result = self.heap.call_attr(heap_id, &attr, args, self.interns);
            obj.drop_with_heap(self.heap);
            result.map(CallResult::Push)
        }
        // ... rest unchanged, but wrap in CallResult::Push
    }
}

/// Handle attribute calls on Path, which may return external calls.
fn call_attr_path(
    &mut self,
    heap_id: HeapId,
    attr: &Attr,
    args: ArgValues,
    obj: Value,
) -> Result<CallResult, RunError> {
    // Take the Path out of the heap temporarily
    let mut data = self.heap.take_data(heap_id);

    let HeapData::Path(ref mut path) = data else {
        unreachable!("call_attr_path called on non-Path");
    };

    let exit = path.py_call_attr_raw(self.heap, attr, args, self.interns)?;

    // Restore data
    self.heap.restore_data(heap_id, data);
    obj.drop_with_heap(self.heap);

    match exit {
        FrameExit::Return(value) => Ok(CallResult::Push(value)),
        FrameExit::ExternalCall { ext_function_id, args, call_id } => {
            Ok(CallResult::External(ext_function_id, args))
        }
        FrameExit::ResolveFutures(_) => {
            Err(ExcType::runtime_error("unexpected ResolveFutures from py_call_attr_raw").into())
        }
    }
}
```

### Step 3: Update `exec_call_attr` Callers

**File:** `crates/monty/src/bytecode/vm/call.rs`

Update `exec_call_attr` to return `CallResult`:

```rust
pub(super) fn exec_call_attr(&mut self, name_id: StringId, arg_count: usize) -> Result<CallResult, RunError> {
    let args = self.pop_n_args(arg_count);
    let obj = self.pop();
    self.call_attr(obj, name_id, args)
}
```

### Step 4: Update VM Main Loop

**File:** `crates/monty/src/bytecode/vm/mod.rs`

Update the handling of `exec_call_attr` in the main loop to handle `CallResult`:

```rust
// Change from:
match self.exec_call_attr(name_id, arg_count) {
    Ok(result) => self.push(result),
    Err(err) => catch_sync!(self, cached_frame, err),
}

// To:
match self.exec_call_attr(name_id, arg_count) {
    Ok(CallResult::Push(result)) => self.push(result),
    Ok(CallResult::FramePushed) => reload_cache!(self, cached_frame),
    Ok(CallResult::External(ext_id, args)) => {
        self.current_frame_mut().ip = cached_frame.ip;
        let call_id = self.allocate_call_id();
        return Ok(FrameExit::ExternalCall {
            ext_function_id: ext_id,
            args,
            call_id,
        });
    }
    Err(err) => catch_sync!(self, cached_frame, err),
}
```

Also update `exec_call_attr_kw` similarly.

### Step 5: Implement `py_call_attr_raw` for Path

**File:** `crates/monty/src/types/path.rs`

Add `py_call_attr_raw` implementation that returns `FrameExit::ExternalCall` for filesystem methods:

```rust
impl Path {
    pub fn py_call_attr_raw(
        &mut self,
        heap: &mut Heap<impl ResourceTracker>,
        attr: &Attr,
        args: ArgValues,
        interns: &Interns,
    ) -> Result<FrameExit, RunError> {
        let static_str = attr.as_static_string();

        match static_str {
            // Filesystem methods - return external call
            Some(StaticStrings::Exists) => {
                args.drop_with_heap(heap);
                let ext_id = interns.get_ext_function_id_by_name("__os_access_exists__")?;
                let path_value = Value::Ref(heap.allocate(HeapData::Str(Str::new(self.path.clone())))?);
                Ok(FrameExit::ExternalCall {
                    ext_function_id: ext_id,
                    args: ArgValues::One(path_value),
                    call_id: CallId::new(0), // Will be set by VM
                })
            }
            Some(StaticStrings::IsFile) => {
                args.drop_with_heap(heap);
                let ext_id = interns.get_ext_function_id_by_name("__os_access_is_file__")?;
                let path_value = Value::Ref(heap.allocate(HeapData::Str(Str::new(self.path.clone())))?);
                Ok(FrameExit::ExternalCall {
                    ext_function_id: ext_id,
                    args: ArgValues::One(path_value),
                    call_id: CallId::new(0),
                })
            }
            // ... similar for is_dir, is_symlink, stat, read_bytes, read_text, iterdir, resolve, absolute

            // Pure methods - delegate to py_call_attr
            _ => {
                let value = self.py_call_attr(heap, attr, args, interns)?;
                Ok(FrameExit::Return(value))
            }
        }
    }
}
```

### Step 6: Add Method to Lookup External Function ID

**File:** `crates/monty/src/intern.rs`

Add a method to lookup `ExtFunctionId` by name:

```rust
impl Interns {
    /// Looks up an external function ID by name.
    ///
    /// Returns an error if the function is not registered.
    pub fn get_ext_function_id_by_name(&self, name: &str) -> Result<ExtFunctionId, RunError> {
        self.external_functions
            .iter()
            .position(|s| s == name)
            .map(ExtFunctionId::new)
            .ok_or_else(|| ExcType::runtime_error(
                format!("external function '{name}' not registered")
            ).into())
    }
}
```

### Step 7: Add External Functions to Test Runner

**File:** `crates/monty/tests/datatest_runner.rs`

Add the `__os_access_*` functions to `ITER_EXT_FUNCTIONS`:

```rust
const ITER_EXT_FUNCTIONS: &[&str] = &[
    // ... existing functions ...

    // Path filesystem functions
    "__os_access_exists__",
    "__os_access_is_file__",
    "__os_access_is_dir__",
    "__os_access_is_symlink__",
    "__os_access_stat__",
    "__os_access_read_bytes__",
    "__os_access_read_text__",
    "__os_access_iterdir__",
    "__os_access_resolve__",
    "__os_access_absolute__",
];
```

Add dispatch handlers in `dispatch_external_call`:

```rust
"__os_access_exists__" => {
    assert!(args.len() == 1);
    let path = String::try_from(&args[0]).expect("path must be str");
    // Mock: paths starting with "/exists" exist
    let exists = path.starts_with("/exists") || path == "/";
    DispatchResult::Sync(MontyObject::Bool(exists).into())
}
"__os_access_is_file__" => {
    let path = String::try_from(&args[0]).expect("path must be str");
    let is_file = path.starts_with("/exists") && !path.ends_with('/');
    DispatchResult::Sync(MontyObject::Bool(is_file).into())
}
// ... etc
```

Also update `scripts/iter_test_methods.py` with Python implementations.

### Step 8: Add Test Cases

**File:** `crates/monty/test_cases/pathlib__filesystem.py` (NEW)

```python
# call-external
from pathlib import Path

# === exists() ===
assert Path('/exists/file.txt').exists() == True
assert Path('/missing/file.txt').exists() == False
assert Path('/').exists() == True

# === is_file() ===
assert Path('/exists/file.txt').is_file() == True
assert Path('/exists/').is_file() == False

# === is_dir() ===
assert Path('/exists/dir/').is_dir() == True
assert Path('/exists/file.txt').is_dir() == False
```

## Files to Modify

| File | Change |
|------|--------|
| `crates/monty/src/types/py_trait.rs` | Add `py_call_attr_raw` method with default impl |
| `crates/monty/src/bytecode/vm/call.rs` | Update `call_attr` to return `CallResult`, add `call_attr_path` |
| `crates/monty/src/bytecode/vm/mod.rs` | Handle `CallResult` from `exec_call_attr` |
| `crates/monty/src/types/path.rs` | Implement `py_call_attr_raw` for filesystem methods |
| `crates/monty/src/intern.rs` | Add `get_ext_function_id_by_name` method |
| `crates/monty/tests/datatest_runner.rs` | Add `__os_access_*` functions and handlers |
| `scripts/iter_test_methods.py` | Add Python implementations for CPython tests |
| `crates/monty/test_cases/pathlib__filesystem.py` | **NEW** - Tests for filesystem methods |

## Key Design Points

1. **`py_call_attr_raw`** - New PyTrait method that returns `FrameExit` directly. Default implementation wraps `py_call_attr` result in `FrameExit::Return`. Types like Path override this to return `FrameExit::ExternalCall` for I/O methods.

2. **`CallResult` return type** - `call_attr` now returns `CallResult` instead of `Value`, allowing it to signal external calls without using error types.

3. **Special handling for Path** - In `call_attr`, detect Path type and use `py_call_attr_raw` instead of the normal `heap.call_attr` path.

4. **External function registration** - Functions must be registered at creation time via `MontyRun::new(..., external_functions)`.

## Verification

1. `make test-cases` - Run pathlib tests (both pure and filesystem)
2. `make lint-rs && make format-rs` - Code quality
3. `make test-ref-count-panic` - Ensure no memory leaks
