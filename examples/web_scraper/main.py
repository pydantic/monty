import asyncio
import re
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

import logfire
import pydantic_core
from pydantic_ai import Agent, ModelRequest, ModelRequestNode, UserPromptPart
from pydantic_graph import End

from pydantic_monty import Monty, MontyError, MontyRuntimeError

from .browser import start_browser
from .external_functions import beautiful_soup
from .sub_agent import RecordModels

logfire.configure()
logfire.instrument_pydantic_ai()

THIS_DIR = Path(__file__).parent


def _generate_stubs() -> str:
    """Generate type stubs for external_functions.py using stubgen."""
    with tempfile.TemporaryDirectory() as tmpdir:
        subprocess.run(
            ['uv', 'run', 'stubgen', 'external_functions.py', '--include-docstrings', '-o', tmpdir],
            capture_output=True,
            text=True,
            cwd=THIS_DIR,
            check=True,
        )
        return (Path(tmpdir) / 'external_functions.pyi').read_text()


stubs = f"""
{RecordModels.record_model_info_stub()}

{_generate_stubs()}
"""
instrunctions = f"""
You MUST return markdown with either a comment and python code to execute
in a "```python" code block, or an explanation of your process to end.

You MUST return only one code block to execute. DO NOT return multiple code blocks.

You MUST use the `record_model_info` function to record information about every model you find.

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
"""

scrape_agent = Agent('gateway/anthropic:claude-sonnet-4-5', instructions=instrunctions)

urls = {
    'openai': 'https://developers.openai.com/api/docs/pricing',
    'anthropic': 'https://platform.claude.com/docs/en/about-claude/pricing',
    'groq': 'https://groq.com/pricing',
}


async def main(model: str):
    url = urls[model]
    prompt = f"""
Get structured information including pricing data for all models from the following URL:

{url}

The HTML returned from this URL is too big for context, so make sure to process it with
the functions provided or return a small snippet of the HTML to process.

Ignore any deprecated models.
"""

    print_output: list[str] = []

    def monty_print(_: Literal['stdout'], content: str):
        print_output.append(content)

    record_models = RecordModels()

    async with start_browser() as browser:
        async with scrape_agent.iter(prompt) as agent_run:
            node = agent_run.next_node
            while True:
                while not isinstance(node, End):
                    node = await agent_run.next(node)

                extracted = ExtractCode.extract(node.data.output)
                logfire.info(f'{extracted}')
                if extracted.comment:
                    print(f'LLM: {extracted.comment}')

                if not extracted.code:
                    print('done')
                    break

                try:
                    with logfire.span('prepare monty', code=extracted.code):
                        m = Monty(
                            extracted.code,
                            type_check=True,
                            type_check_stubs=stubs,
                        )
                except MontyError as e:
                    msg = f'Error Preparing Code: {e}'
                    node = await agent_run.next(new_node(msg))
                    continue

                try:
                    with logfire.span('running monty'):
                        output = await m.run_async(
                            external_functions={
                                'open_page': browser.open_page,
                                'beautiful_soup': beautiful_soup,
                                'record_model_info': record_models.record_model_info,
                            },
                            print_callback=monty_print,
                        )
                except MontyRuntimeError as e:
                    msg = f'Error running code: {e.display()}'
                else:
                    msg = pydantic_core.to_json(output).decode()

                if print_output:
                    msg += f'\n\nPrint Output:\n---\n{"".join(print_output)}\n---'
                    print_output.clear()
                node = await agent_run.next(new_node(msg))

        logfire.info('{models=}', models=record_models.models)


def new_node(msg: str) -> ModelRequestNode[None, str]:
    return ModelRequestNode(request=ModelRequest(instructions=instrunctions, parts=[UserPromptPart(content=msg)]))


@dataclass
class ExtractCode:
    """Extract Python code from an LLM response.

    Priority:
    1. First ```python code fence
    2. First code fence of any language
    3. Entire response as-is
    """

    code: str | None
    """Code extract from response.

    First ```(python|py) code fence, or first unexplained

    """
    comment: str | None

    @classmethod
    def extract(cls, response: str) -> ExtractCode:
        # Try ```python or ```py fences first
        m = re.search(r'```(?:python|py)\s*\n(.*?)```', response, re.DOTALL)
        if not m:
            # Try any code fence
            m = re.search(r'```\w*\s*\n(.*?)```', response, re.DOTALL)

        if m:
            code = m.group(1).strip()
            # Extract comment as text before the code fence
            comment = response[: m.start()].strip() or None
            return cls(code=code, comment=comment)

        return cls(code=None, comment=response.strip())


if __name__ == '__main__':
    asyncio.run(main('anthropic'))
