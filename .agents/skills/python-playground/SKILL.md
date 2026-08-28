---
name: python-playground
description: Run and test Python code in a dedicated playground directory. Use when you need to execute Python scripts, test code snippets, investigate CPython behavior, or experiment with Python without affecting the main codebase.
---

# Python Playground

Run Python code in an isolated playground directory for testing and experimentation.

## Instructions

1. First, ensure the playground directory exists. If the `playground` directory doesn't already exist, run `mkdir playground`.
2. Use the Write tool to create the Python file at `playground/test.py`
3. Run with: `uv run playground/test.py` to test cpython behavior or `cargo run -- playground/test.py` to test monty behavior

IMPORTANT: Use separate tool calls for each step - do NOT chain commands with `&&`. This allows the pre-approved commands to work without prompting.

## Example workflow

Step 1 - Create directory if it doesn't already exist (Bash, already allowed):

```bash
mkdir playground
```

Step 2 - Write code (use Write tool, not cat):
Write to `playground/test.py`:

```python
def foo():
    raise ValueError('test')

foo()
```

Step 3 - Run script (Bash, already allowed):

```bash
uv run playground/test.py
```

## Guidelines

- The `playground/` directory is gitignored
- Use a different file name for each test you want to run, give the files recognizable names like `test_value_error.py`
- Use `uv run ...` to run scripts (uses project Python)
- Or, `cargo run -- ...` to run scripts using Monty
- Use Write tool for creating files (avoids permission prompts)
- Run mkdir and uv as separate commands (not chained)
- do NOT delete files from playground after you've finished testing
