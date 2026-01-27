# Monty

[![CodSpeed](https://img.shields.io/badge/CodSpeed-Performance%20Tracked-blue?logo=data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMTYiIGhlaWdodD0iMTYiIHZpZXdCb3g9IjAgMCAxNiAxNiIgZmlsbD0ibm9uZSIgeG1sbnM9Imh0dHA6Ly93d3cudzMub3JnLzIwMDAvc3ZnIj48cGF0aCBkPSJNOCAwTDAgOEw4IDE2TDE2IDhMOCAwWiIgZmlsbD0id2hpdGUiLz48L3N2Zz4=)](https://codspeed.io/pydantic/monty?utm_source=badge)
[![ci](https://github.com/pydantic/monty/actions/workflows/ci.yml/badge.svg)](https://github.com/pydantic/monty/actions/workflows/ci.yml)

**Experimental** - This project is still in development, and not ready for the prime time.

A minimal, secure Python interpreter written in Rust for use by AI.

Monty avoids the cost, latency, complexity and general faff of using full container based sandbox for running LLM generated code.

Instead, it let's you run safely run Python code written by an LLM embedded in your agent, with startup times measured in single digit microseconds not hundreds of milliseconds.

What Monty **can** do:
* Run a reasonable subset of Python code - enough for your agent to express that it wants to do
* Completely block access to the host environment: no filesystem, env variables or network access
* Call functions on the host - only functions you give it access to
* Run typechecking - monty supports full modern python type hints and comes with [ty](https://docs.astral.sh/ty/) including in a single binary to run typechecking
* Be snapshotted to bytes at external function calls, meaning you can store the interpreter state in a file or database, and resume later
* Startup extremely fast (<1μs to go from code to execution result), and has runtime performance that is similar to CPython (generally between 5x faster and 5x slower)
* Be called from Rust, Python, or Javascript - because Monty has no dependencies on cpython, you can use it anywhere you can run Rust
* Control resource usage - Monty can track memory usage, allocations, stack depth, and execution time and cancel execution if it exceeds preset limits
* Collect stdout and stderr and return it to the caller

What Monty **cannot** do:
* Use the standard library (except a few select modules: `sys`, `typing`, `asyncio`, `dataclasses` (soon), `json` (soon))
* Use third party libraries (like Pydantic), support for external python library is not a goal
* define classes (support should come soon)
* use match statements (again, support should come soon)

---

In short, Monty is extremely limited and designed for **one** use case:

**To run code written by agents.**

For motivation on why you might want to do this, see:
* [Codemode](https://blog.cloudflare.com/code-mode/) from Cloudflare
* [Programmatic Tool Calling](https://platform.claude.com/docs/en/agents-and-tools/tool-use/programmatic-tool-calling) from Anthropic
* [Code Execution with MCP](https://www.anthropic.com/engineering/code-execution-with-mcp) from Anthropic
* [Smol Agents](https://github.com/huggingface/smolagents) from Hugging Face

In very simple terms, the idea of all the above is that LLMs can be work faster, cheaper and more reliably if they're asked to write Python (or Javascript) code, instead of relying on traditional tool calling. Monty makes that possible without the complexity of a sandbox or risk of running code directly on the host.

**Note:** Monty will (soon) be used to implement `codemode` in [Pydantic AI](https://github.com/pydantic/pydantic-ai)

## Usage

Monty can be called from Python, JavaScript/TypeScript or Rust.

### Python

To install:

```bash
uv add pydantic-monty
```

(Or `pip install pydantic-monty` for the boomers)

Usage:

```python
import pydantic_monty

code = """
def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)

fib(x)
"""

m = pydantic_monty.Monty(code, inputs=['x'], script_name='fib.py')
print(m.run(inputs={'x': 10}))
#> 55
```

#### Iterative Execution with External Functions

Use `start()` and `resume()` to handle external function calls iteratively,
giving you control over each call:

```python
import pydantic_monty

code = """
data = fetch(url)
len(data)
"""

m = pydantic_monty.Monty(code, inputs=['url'], external_functions=['fetch'])

# Start execution - pauses when fetch() is called
result = m.start(inputs={'url': 'https://example.com'})

print(type(result))
#> <class 'pydantic_monty.MontySnapshot'>
print(result.function_name)  # fetch
#> fetch
print(result.args)
#> ('https://example.com',)

# Perform the actual fetch, then resume with the result
result = result.resume(return_value='hello world')

print(type(result))
#> <class 'pydantic_monty.MontyComplete'>
print(result.output)
#> 11
```

#### Serialization

Both `Monty` and `MontySnapshot` can be serialized to bytes and restored later.
This allows caching parsed code or suspending execution across process boundaries:

```python
import pydantic_monty

# Serialize parsed code to avoid re-parsing
m = pydantic_monty.Monty('x + 1', inputs=['x'])
data = m.dump()

# Later, restore and run
m2 = pydantic_monty.Monty.load(data)
print(m2.run(inputs={'x': 41}))
#> 42

# Serialize execution state mid-flight
m = pydantic_monty.Monty('fetch(url)', inputs=['url'], external_functions=['fetch'])
progress = m.start(inputs={'url': 'https://example.com'})
state = progress.dump()

# Later, restore and resume (e.g., in a different process)
progress2 = pydantic_monty.MontySnapshot.load(state)
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

## What Python Syntax is supported
