"""A Pydantic AI agent that drives Chrome by writing Python, which Monty runs in a sandbox.

`CodeMode` replaces the `browser_cdp` tool with one `run_code` tool: the model writes Python that
calls `browser_cdp` as a function, and that code runs in Monty, so it can browse but cannot touch
the host otherwise.
"""

from __future__ import annotations

import asyncio
import sys

import logfire
from pydantic_ai import Agent
from pydantic_ai_harness import CodeMode

from .cdp import CDPSession, browser_cdp, launch_chrome

logfire.configure(service_name='browser-cdp')
logfire.instrument_pydantic_ai()

agent = Agent(
    'gateway/openai:gpt-6.1-sol',
    deps_type=CDPSession,
    tools=[browser_cdp],
    instructions="""
You control a Chrome tab by sending Chrome DevTools Protocol commands with `browser_cdp`.
Pages are big: extract only what you need with `Runtime.evaluate` rather than returning the whole HTML.
""",
    capabilities=[CodeMode()],
)


async def main(task: str, headless: bool) -> None:
    async with launch_chrome(headless=headless) as session:
        result = await agent.run(task, deps=session)
    print(result.output)


if __name__ == '__main__':
    default_task = 'Get the API prices for all claude modeles from their website.'
    args = sys.argv[1:]
    # `--headed` opens a visible Chrome window so you can watch the agent browse
    headed = '--headed' in args
    task = ' '.join(a for a in args if a != '--headed') or default_task
    asyncio.run(main(task, headless=not headed))
