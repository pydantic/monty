<div align="center">
  <h1>Monty</h1>
</div>
<div align="center">
  <h3>A minimal, secure Python interpreter written in Rust for use by AI.</h3>
</div>
<div align="center">
  <a href="https://github.com/pydantic/monty/actions/workflows/ci.yml?query=branch%3Amain"><img src="https://github.com/pydantic/monty/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://codspeed.io/pydantic/monty?utm_source=badge"><img src="https://img.shields.io/badge/CodSpeed-Performance%20Tracked-blue?logo=data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMTYiIGhlaWdodD0iMTYiIHZpZXdCb3g9IjAgMCAxNiAxNiIgZmlsbD0ibm9uZSIgeG1sbnM9Imh0dHA6Ly93d3cudzMub3JnLzIwMDAvc3ZnIj48cGF0aCBkPSJNOCAwTDAgOEw4IDE2TDE2IDhMOCAwWiIgZmlsbD0id2hpdGUiLz48L3N2Zz4=" alt="Codspeed"></a>
  <a href="https://codecov.io/gh/pydantic/monty"><img src="https://codecov.io/gh/pydantic/monty/graph/badge.svg?token=HX4RDQX5OG" alt="Coverage"></a>
  <a href="https://pypi.python.org/pypi/pydantic-monty"><img src="https://img.shields.io/pypi/v/pydantic-monty.svg" alt="PyPI"></a>
  <a href="https://github.com/pydantic/monty"><img src="https://img.shields.io/pypi/pyversions/pydantic-monty.svg" alt="versions"></a>
  <a href="https://github.com/pydantic/monty/blob/main/LICENSE"><img src="https://img.shields.io/github/license/pydantic/monty.svg?v=2" alt="license"></a>
  <a href="https://logfire.pydantic.dev/docs/join-slack/"><img src="https://img.shields.io/badge/Slack-Join%20Slack-4A154B?logo=slack" alt="Join Slack" /></a>
</div>

---

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
* Run async or sync code on the host via async or sync code on the host

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
from typing import Any

import pydantic_monty

code = """
async def agent(prompt: str, messages: Messages):
    while True:
        print(f'messages so far: {messages}')
        output = await call_llm(prompt, messages)
        if isinstance(output, str):
            return output
        messages.extend(output)

await agent(prompt, [])
"""

type_definitions = """
from typing import Any

Messages = list[dict[str, Any]]

async def call_llm(prompt: str, messages: Messages) -> str | Messages:
    raise NotImplementedError()

prompt: str = ''
"""

m = pydantic_monty.Monty(
    code,
    inputs=['prompt'],
    external_functions=['call_llm'],
    script_name='agent.py',
    type_check=True,
    type_check_stubs=type_definitions,
)


Messages = list[dict[str, Any]]


async def call_llm(prompt: str, messages: Messages) -> str | Messages:
    if len(messages) < 2:
        return [{'role': 'system', 'content': 'example response'}]
    else:
        return f'example output, message count {len(messages)}'


async def main():
    output = await pydantic_monty.run_monty_async(
        m,
        inputs={'prompt': 'testing'},
        external_functions={'call_llm': call_llm},
    )
    print(output)
    #> example output, message count 2


if __name__ == '__main__':
    import asyncio

    asyncio.run(main())
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

## PydanticAI Integration

Monty will power code-mode in
[Pydantic AI](https://github.com/pydantic/pydantic-ai). Instead of making
sequential tool calls, the LLM writes Python code that calls your tools
as functions and Monty executes it safely.

```python test="skip"
from pydantic_ai import Agent
from pydantic_ai.toolsets.code_mode import CodeModeToolset
from pydantic_ai.toolsets.function import FunctionToolset
from typing_extensions import TypedDict


class WeatherResult(TypedDict):
    city: str
    temp_c: float
    conditions: str


toolset = FunctionToolset()


@toolset.tool
def get_weather(city: str) -> WeatherResult:
    """Get current weather for a city."""
    # your real implementation here
    return {'city': city, 'temp_c': 18, 'conditions': 'partly cloudy'}


@toolset.tool
def get_population(city: str) -> int:
    """Get the population of a city."""
    return {'london': 9_000_000, 'paris': 2_100_000, 'tokyo': 14_000_000}.get(
        city.lower(), 0
    )


toolset = CodeModeToolset(toolset)

agent = Agent(
    'anthropic:claude-sonnet-4-5',
    toolsets=[toolset],
)

result = agent.run_sync(
    'Compare the weather and population of London, Paris, and Tokyo.'
)
print(result.output)
```

# Alternatives

There are generally two responses when you show people Monty:

1. Oh my god, this solves so many problems, I want it.
2. Why not X?

Where X is some alternative technology. Oddly often these responses are combined, suggesting people have not yet found an alternative that works for them, but are incredulous that there's really no good alternative to creating an entire Python implementation from scratch.

I'll try to run through the most obvious alternatives, and why there aren't right for what we wanted.

NOTE: all these technologies are impressive and have widespread uses, this commentary on their limitations for our use case should not be seen as a criticism. Most of these solutions were not conceived with the goal of providing an LLM sandbox, which is why they're not necessary great at it.

| Tech               | Language completeness | Security     | Start latency  | Cost     | Setup complexity | File mounting  | Snapshotting |
|--------------------|-----------------------|--------------|----------------|----------|------------------|----------------|--------------|
| Monty              | partial               | strict       | 0.06ms         | free     | easy             | easy           | easy         |
| Docker             | full                  | good         | 195ms          | free     | intermediate     | easy           | intermediate |
| Pyodide            | full                  | poor         | 2800ms         | free     | intermediate     | easy           | hard         |
| starlark-rust      | very limited          | good         | 1.7ms          | free     | easy             | not available? | impossible?  |
| sandboxing service | full                  | strict       | 1033ms         | not free | intermediate     | hard           | intermediate |
| YOLO Python        | full                  | non-existent | 0.1ms / 30ms   | free     | easy             | easy / scary   | hard         |

See [./scripts/startup_performance.py](scripts/startup_performance.py) for the script used to calculate the startup performance numbers.

Details on each row below:

### Monty

- **Language completeness**: No classes (yet), limited stdlib, no third-party libraries
- **Security**: Explicitly controlled filesystem, network, and env access, strict limits on execution time and memory usage
- **Start latency**: Starts in microseconds
- **Setup complexity**: just `pip install pydantic-monty` or `npm install @pydantic/monty`, ~4.5MB download
- **File mounting**: Strictly controlled, see [#85](https://github.com/pydantic/monty/pull/85)
- **Snapshotting**: Monty's pause and resume functionality with `dump()` and `load()` makes it trivial to pause, resume and fork execution

### Docker

- **Language completeness**: Full CPython with any library
- **Security**: Process and filesystem isolation, network policies, but container escapes exist, memory limitation is possible
- **Start latency**: Container startup overhead (~195ms measured)
- **Setup complexity**: Requires Docker daemon, container images, orchestration, `python:3.14-alpine` is 50MB - docker can't be installed from PyPI
- **File mounting**: Volume mounts work well
- **Snapshotting**: Possible with durable execution solutions like Temporal, or snapshotting an image and saving it as a Docker image.

### Pyodide

- **Language completeness**: Full CPython compiled to WASM, almost all libraries available
- **Security**: Relies on browser/WASM sandbox - not designed for server-side isolation, python code can run arbitrary code in the JS runtime, only deno allows isolation, memory limits are hard/impossible to enforce with deno
- **Start latency**: WASM runtime loading is slow (~2800ms cold start)
- **Setup complexity**: Need to load WASM runtime, handle async initialization, pyodide NPM package is ~12MB, deno is ~50MB - Pyodide can't be called with just PyPI packages
- **File mounting**: Virtual filesystem via browser APIs
- **Snapshotting**: Possible with durable execution solutions like Temporal presumably, but hard

### starlark-rust

See [starlark-rust](https://github.com/facebook/starlark-rust).

- **Language completeness**: Configuration language, not Python - no classes, exceptions, async
- **Security**: Deterministic and hermetic by design
- **Start latency**: runs embedded in the process like Monty, hence impressive startup time
- **Setup complexity**: Usable in python via [starlark-pyo3](https://github.com/inducer/starlark-pyo3)
- **File mounting**: No file handling by design AFAIK?
- **Snapshotting**: Impossible AFAIK?

### sandboxing service

Services like [Daytona](https://daytona.io), [E2B](https://e2b.dev), [Modal](https://modal.com).

There are similar challenges, more setup complexity but lower network latency for setting up your own sandbox setup with k8s.

- **Language completeness**: Full CPython with any library
- **Security**: Professionally managed container isolation
- **Start latency**: Network round-trip and container startup time. I got ~1s cold start time with Daytona EU from London, Daytona advertise sub 90ms latency, presumably that's for an existing container, not clear if it includes network latency
- **Cost**: Pay per execution or compute time
- **Setup complexity**: API integration, auth tokens - fine for startups but generally a non-start for enterprises
- **File mounting**: Upload/download via API calls
- **Snapshotting**: Possible with durable execution solutions like Temporal, also the services offer some solutions for this, I think based con docker containers

### YOLO Python

Running Python directly via `exec()` (~0.1ms) or subprocess (~30ms).

- **Language completeness**: Full CPython with any library
- **Security**: None - full filesystem, network, env vars, system commands
- **Start latency**: Near-zero for `exec()`, ~30ms for subprocess
- **Setup complexity**: None
- **File mounting**: Direct filesystem access (that's the problem)
- **Snapshotting**: Possible with durable execution solutions like Temporal
