# pydantic-monty

Python bindings for the Monty sandboxed Python interpreter.

## Installation

```bash
pip install pydantic-monty
```

## Usage

### Basic Expression Evaluation

```python
import pydantic_monty

# Simple code with no inputs
m = pydantic_monty.Monty('1 + 2')
print(m.run())
#> 3
```

### Using Input Variables

```python
import pydantic_monty

# Create with code that uses input variables
m = pydantic_monty.Monty('x * y', inputs=['x', 'y'])

# Run multiple times with different inputs
print(m.run(inputs={'x': 2, 'y': 3}))
#> 6
print(m.run(inputs={'x': 10, 'y': 5}))
#> 50
```

### Resource Limits

```python
import pydantic_monty

m = pydantic_monty.Monty('x + y', inputs=['x', 'y'])

# With resource limits
limits = pydantic_monty.ResourceLimits(max_duration_secs=1.0)
result = m.run(inputs={'x': 1, 'y': 2}, limits=limits)
assert result == 3
```

### External Functions

```python
import pydantic_monty

# Code that calls an external function
m = pydantic_monty.Monty('double(x)', inputs=['x'], external_functions=['double'])

# Provide the external function implementation at runtime
result = m.run(inputs={'x': 5}, external_functions={'double': lambda x: x * 2})
print(result)
#> 10
```

### Iterative Execution with External Functions

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

### Serialization

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
```

Execution state can also be serialized mid-flight:

```python
import pydantic_monty

m = pydantic_monty.Monty('fetch(url)', inputs=['url'], external_functions=['fetch'])
progress = m.start(inputs={'url': 'https://example.com'})

# Serialize the execution state
state = progress.dump()

# Later, restore and resume (e.g., in a different process)
progress2 = pydantic_monty.MontySnapshot.load(state)
result = progress2.resume(return_value='response data')
print(result.output)
#> response data
```
