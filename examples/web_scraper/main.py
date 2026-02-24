import asyncio
from pathlib import Path

import logfire
from pydantic_ai import Agent

logfire.configure()

THIS_DIR = Path(__file__).parent
stubs = THIS_DIR / 'stubs.pyi'

scrape_agent = Agent(
    'gateway/anthropic:claude-sonnet-4-5',
    instructions=f"""
You MUST return only python code to execute the task you are working on.

The runtime uses a restricted Python subset:
- you cannot use the standard library except builtin functions and the following modules: `sys`, `typing`, `asyncio`
- this means `collections`, `json`, `re`, `math`, `datetime`, `itertools`, `functools`, etc. are NOT available — use plain dicts, lists, and builtins instead
- you cannot use third party libraries
- you cannot define classes

The last expression evaluated is the return value.

You can also `print()` values

Parallelism: use `asyncio.gather` to fire multiple calls at the same time instead of awaiting each one sequentially:

```python
import asyncio

# GOOD — parallel (all calls fire at once):
results = await asyncio.gather(
    get_data(id=1),
    get_data(id=2),
    get_data(id=3),
)

# BAD — sequential (each call waits before the next starts):
r1 = await get_data(id=1)
r2 = await get_data(id=2)
r3 = await get_data(id=3)
```

You can use the following types functions and types:

```python
{stubs.read_text()}
```
""",
)

urls = {
    'openai': 'https://developers.openai.com/api/docs/pricing',
    'anthropic': 'https://platform.claude.com/docs/en/about-claude/pricing',
    'groq': 'https://groq.com/pricing',
}


async def main(model: str):
    url = urls[model]
    result = await scrape_agent.run(
        f"""
Get structured information including pricing data for all models from the following URL:

{url}

The HTML returned from this URL is too big for context, so make sure to process it with
the functions provided or return a small snippet of the HTML to process.

Ignore any deprecated models.
"""
    )
    print(result.output)


if __name__ == '__main__':
    asyncio.run(main('openai'))
