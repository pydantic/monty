# Examples

## Code Mode in Pydantic AI

[Pydantic AI](https://github.com/pydantic/pydantic-ai) runs Monty behind
[`CodeModeToolset`](https://pydantic.dev/docs/ai/harness/code-mode/).
Instead of making sequential tool calls, the model writes Python that calls your tools as functions, and Monty executes
it: one round trip for a task that would otherwise take three.

```python test="skip"
import asyncio
import json

import logfire
from httpx import AsyncClient
from pydantic_ai import Agent, RunContext
from pydantic_ai.toolsets.code_mode import CodeModeToolset
from pydantic_ai.toolsets.function import FunctionToolset
from typing_extensions import TypedDict

logfire.configure()
logfire.instrument_pydantic_ai()


class LatLng(TypedDict):
    lat: float
    lng: float


weather_toolset: FunctionToolset[AsyncClient] = FunctionToolset()


@weather_toolset.tool
async def get_lat_lng(
    ctx: RunContext[AsyncClient], location_description: str
) -> LatLng:
    """Get the latitude and longitude of a location."""
    # NOTE: the response here will be random, and is not related to the location description.
    r = await ctx.deps.get(
        'https://demo-endpoints.pydantic.workers.dev/latlng',
        params={'location': location_description},
    )
    r.raise_for_status()
    return json.loads(r.content)


@weather_toolset.tool
async def get_temp(ctx: RunContext[AsyncClient], lat: float, lng: float) -> float:
    """Get the temp at a location."""
    # NOTE: the responses here will be random, and are not related to the lat and lng.
    r = await ctx.deps.get(
        'https://demo-endpoints.pydantic.workers.dev/number',
        params={'min': 10, 'max': 30},
    )
    r.raise_for_status()
    return float(r.text)


@weather_toolset.tool
async def get_weather_description(
    ctx: RunContext[AsyncClient], lat: float, lng: float
) -> str:
    """Get the weather description at a location."""
    # NOTE: the responses here will be random, and are not related to the lat and lng.
    r = await ctx.deps.get(
        'https://demo-endpoints.pydantic.workers.dev/weather',
        params={'lat': lat, 'lng': lng},
    )
    r.raise_for_status()
    return r.text


agent = Agent(
    'gateway/anthropic:claude-sonnet-4-5',
    toolsets=[CodeModeToolset(weather_toolset)],
    deps_type=AsyncClient,
)


async def main():
    async with AsyncClient() as client:
        await agent.run('Compare the weather of London, Paris, and Tokyo.', deps=client)


if __name__ == '__main__':
    asyncio.run(main())
```

Swap `CodeModeToolset(weather_toolset)` for `weather_toolset` to see the same task done with ordinary tool calls.

## Worked examples in the repository

Each directory under [`examples/`](https://github.com/pydantic/monty/tree/main/examples) is runnable after `make dev-py`; its README has the command.

- [`sql_playground`](https://github.com/pydantic/monty/tree/main/examples/sql_playground): customer purchase data in CSV
    joined with tweets in JSON, with sentiment analysis called in a loop from the sandbox.
    With JSON tool calling the 50+ per-tweet results would flood the context window; in Monty they stay inside the sandbox
    and only the aggregate comes out.
    Also shows file sandboxing via the `os` callback and type checking against a stub file.
- [`expense_analysis`](https://github.com/pydantic/monty/tree/main/examples/expense_analysis): Anthropic's [programmatic
    tool calling](https://platform.claude.com/cookbook/tool-use-programmatic-tool-calling-ptc) cookbook example, run on
    Monty.
- [`web_scraper`](https://github.com/pydantic/monty/tree/main/examples/web_scraper): Playwright and BeautifulSoup
    exposed to the sandbox as [host objects](host-objects.md) so the model can extract prices from model labs' websites;
    `example_code.py` is the code Claude Sonnet 4.5 wrote for it.
- [`classes`](https://github.com/pydantic/monty/tree/main/examples/classes): one short file per behaviour of [host
    objects](host-objects.md), in Python and TypeScript: explicit policies, lazy attributes, sandbox-side copies,
    `convert_value` hooks, constructing host classes from the sandbox, and round-tripping sandbox-defined classes.
