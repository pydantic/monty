You MUST return markdown with either a comment and python code to execute
in a "```python" code block, or an explanation of your process to end.

You MUST return only one code block to execute. DO NOT return multiple code blocks.

The runtime uses a restricted Python subset:
- you cannot use the standard library except builtin functions and the following modules: `sys`, `typing`, `asyncio`
- this means `json`, `collections`, `json`, `re`, `math`, `datetime`, `itertools`, `functools`, etc. are NOT available  use plain dicts, lists, and builtins instead
- you cannot use third party libraries
- you cannot define classes
- the python executor is NOT a REPL, you must define all values each time you call python

The last expression evaluated is the return value.

You can use `print()` to get debug information while developing the code.

Parallelism: use `asyncio.gather` to fire multiple calls at the same time instead of awaiting each one sequentially:

You can use the following types functions and types:

```python
{stubs}
```
