"""The model side: turn a prompt and a task into a code block.

Kept separate from the runner so `--dry-run` can substitute a task's checked-in
`reference_solution` for a real model call, which makes the harness runnable with no
API key.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path

from pydantic_ai import Agent
from pydantic_ai.messages import ModelMessage

__all__ = ('CodeAgent', 'DryRunAgent', 'Reply', 'extract_code', 'load_prompt')

PROMPTS_DIR = Path(__file__).parent.parent / 'prompts'

_FENCE = re.compile(r'```(?:python|py)\s*\n(.*?)```', re.DOTALL)


def load_prompt(variant: str) -> str:
    """Read a prompt variant by name, e.g. `v4_codemode`.

    Prompts are files rather than string constants so an optimiser can generate and
    score new ones without touching the harness.
    """
    path = PROMPTS_DIR / f'{variant}.md'
    if not path.is_file():
        available = ', '.join(sorted(p.stem for p in PROMPTS_DIR.glob('*.md')))
        raise FileNotFoundError(f'unknown prompt variant {variant!r}; available: {available}')
    return path.read_text()


def extract_code(text: str) -> str | None:
    """Pull the first ```python block out of a model reply.

    `None` means the model returned prose with no code, which every prompt variant
    defines as "I am finished" — the runner treats it as the end of the attempt.
    """
    match = _FENCE.search(text)
    if match is None:
        return None
    return match.group(1).strip() or None


@dataclass
class Reply:
    """One model turn: the code it wants run, plus what it cost."""

    code: str | None
    prompt_tokens: int = 0
    completion_tokens: int = 0
    text: str = ''


@dataclass
class CodeAgent:
    """Wraps a pydantic-ai `Agent`, holding message history across turns of one attempt.

    History matters in the agentic mode: the model sees its own failed code and the
    resulting Monty traceback, which is what `turns_to_success` measures.
    """

    model: str
    system_prompt: str
    _agent: Agent[None, str] = field(init=False)
    _history: list[ModelMessage] = field(default_factory=list, init=False)

    def __post_init__(self) -> None:
        self._agent = Agent(self.model, instructions=self.system_prompt, output_type=str)

    async def respond(self, user_text: str) -> Reply:
        """Send one user turn and return the code the model wants executed."""
        result = await self._agent.run(user_text, message_history=self._history)
        self._history = list(result.all_messages())
        usage = result.usage
        return Reply(
            code=extract_code(result.output),
            prompt_tokens=usage.input_tokens or 0,
            completion_tokens=usage.output_tokens or 0,
            text=result.output,
        )


@dataclass
class DryRunAgent:
    """Replays a task's reference solution instead of calling a model.

    Returns the solution once and then nothing, so a dry run is always exactly one
    turn: if the reference solution fails, that is a defect in the task or a real
    Monty gap, and either way retrying it would only hide the problem.
    """

    solutions: list[str]
    _index: int = field(default=0, init=False)

    async def respond(self, user_text: str) -> Reply:
        if self._index >= len(self.solutions):
            return Reply(code=None, text='dry run exhausted')
        code = self.solutions[self._index]
        self._index += 1
        return Reply(code=code, text=f'```python\n{code}\n```')
