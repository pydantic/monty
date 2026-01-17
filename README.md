# Monty

[![CodSpeed](https://img.shields.io/badge/CodSpeed-Performance%20Tracked-blue?logo=data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMTYiIGhlaWdodD0iMTYiIHZpZXdCb3g9IjAgMCAxNiAxNiIgZmlsbD0ibm9uZSIgeG1sbnM9Imh0dHA6Ly93d3cudzMub3JnLzIwMDAvc3ZnIj48cGF0aCBkPSJNOCAwTDAgOEw4IDE2TDE2IDhMOCAwWiIgZmlsbD0id2hpdGUiLz48L3N2Zz4=)](https://codspeed.io/pydantic/monty?utm_source=badge)

A sandboxed, snapshotable Python interpreter written in Rust.

Monty is a **sandboxed Python interpreter** written in Rust. Unlike embedding CPython or using PyO3,
Monty implements its own runtime from scratch.

The goal is to provide:
* complete safety - no access to the host environment, filesystem or network
* safe access to specific methods on the host
* snapshotting and iterative execution for long running host functions

## Usage

### Python

```python
import monty

code = """
def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)

fib(x)
"""

m = monty.Monty(code, inputs=['x'], script_name='fib.py')
print(m.run(inputs={'x': 10}))
#> 55
```

#### Iterative Execution with External Functions

Use `start()` and `resume()` to handle external function calls iteratively,
giving you control over each call:

```python
import monty

code = """
data = fetch(url)
len(data)
"""

m = monty.Monty(code, inputs=['url'], external_functions=['fetch'])

# Start execution - pauses when fetch() is called
result = m.start(inputs={'url': 'https://example.com'})

print(type(result))
#> <class 'monty.MontySnapshot'>
print(result.function_name)  # fetch
#> fetch
print(result.args)
#> ('https://example.com',)

# Perform the actual fetch, then resume with the result
result = result.resume(return_value='hello world')

print(type(result))
#> <class 'monty.MontyComplete'>
print(result.output)
#> 11
```

#### Serialization

Both `Monty` and `MontySnapshot` can be serialized to bytes and restored later.
This allows caching parsed code or suspending execution across process boundaries:

```python
import monty

# Serialize parsed code to avoid re-parsing
m = monty.Monty('x + 1', inputs=['x'])
data = m.dump()

# Later, restore and run
m2 = monty.Monty.load(data)
print(m2.run(inputs={'x': 41}))
#> 42

# Serialize execution state mid-flight
m = monty.Monty('fetch(url)', inputs=['url'], external_functions=['fetch'])
progress = m.start(inputs={'url': 'https://example.com'})
state = progress.dump()

# Later, restore and resume (e.g., in a different process)
progress2 = monty.MontySnapshot.load(state)
result = progress2.resume(return_value='response data')
print(result.output)
#> response data
```

### Rust

```rust
use monty::{MontyRun, MontyObject, NoLimitTracker, StdPrint};

let code = r#"
def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)

fib(x)
"#;

let runner = MontyRun::new(code.to_owned(), "fib.py", vec!["x".to_owned()], vec![]).unwrap();
let result = runner.run(vec![MontyObject::Int(10)], NoLimitTracker, &mut StdPrint).unwrap();
assert_eq!(result, MontyObject::Int(55));
```

#### Serialization

`MontyRun` and `RunProgress` can be serialized using the `dump()` and `load()` methods:

```rust
use monty::{MontyRun, MontyObject, NoLimitTracker, StdPrint};

// Serialize parsed code
let runner = MontyRun::new("x + 1".to_owned(), "main.py", vec!["x".to_owned()], vec![]).unwrap();
let bytes = runner.dump().unwrap();

// Later, restore and run
let runner2 = MontyRun::load(&bytes).unwrap();
let result = runner2.run(vec![MontyObject::Int(41)], NoLimitTracker, &mut StdPrint).unwrap();
assert_eq!(result, MontyObject::Int(42));
```
