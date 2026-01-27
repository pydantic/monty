"""Example usage of the Monty Python bindings."""

import pydantic_monty

# Basic execution - simple expression
m = pydantic_monty.Monty('1 + 2 * 3')
print(f'Basic: {m.run()!r}')  # 7

# Using input variables
m = pydantic_monty.Monty('x + y', inputs=['x', 'y'])
print(f'Inputs: {m.run(inputs={"x": 10, "y": 20})}')  # 30

# Reusing the same parsed code with different values
print(f'Reuse: {m.run(inputs={"x": 100, "y": 200})}')  # 300

# With resource limits
limits = pydantic_monty.ResourceLimits(max_duration_secs=5.0, max_memory=1024 * 1024)
m = pydantic_monty.Monty('x * y * z', inputs=['x', 'y', 'z'])
print(f'With limits: {m.run(inputs={"x": 2, "y": 3, "z": 4}, limits=limits)}')  # 24

# External function callbacks
m = pydantic_monty.Monty('fetch("https://example.com")', external_functions=['fetch'])


def fetch(url: str) -> str:
    return f'Fetched: {url}'


print(f'External: {m.run(external_functions={"fetch": fetch})}')

# Print output is forwarded to Python stdout
m = pydantic_monty.Monty('print("Hello from Monty!")')
m.run()

# Exception handling
m = pydantic_monty.Monty('1 / 0')
try:
    m.run()
except ZeroDivisionError as e:
    print(f'Caught: {type(e).__name__}')
