"""Twenty questions with the questioner loop in the sandbox (the PyCon Italy demo).

Sandboxed code holds the game state, asks `call_llm` for the next question, puts it
to `ask_oracle`, and stops when a guess comes back `yes` or the budget runs out.
The oracle answers from a keyword table so the game is deterministic; the scripted
`call_llm` stub narrows in five questions and guesses on the sixth.
"""

from __future__ import annotations

from evals.harness.task import Task

SECRET = 'kettle'

_ORACLE = (
    ('alive', 'no'),
    ('kitchen', 'yes'),
    ('electric', 'yes'),
    ('heat', 'yes'),
    ('water', 'yes'),
    ('kettle', 'yes'),
    ('toaster', 'no'),
    ('fridge', 'no'),
)

_SCRIPT = (
    'Is it alive?',
    'Is it found in a kitchen?',
    'Is it electric?',
    'Does it heat something?',
    'Does it hold water?',
    'Is it a kettle?',
)


async def ask_oracle(question: str) -> str:
    """Host: the other player, who answers about the secret object."""
    lowered = question.lower()
    for keyword, answer in _ORACLE:
        if keyword in lowered:
            return answer
    return 'no'


def _stub_call_llm(prompt: str) -> str:
    """Scripted questioner: the next question is the one after the last `assistant:` line."""
    asked = sum(1 for line in prompt.split('\n') if line.startswith('assistant: '))
    return _SCRIPT[min(asked, len(_SCRIPT) - 1)]


STUBS = '''
async def call_llm(messages: list[dict[str, str]]) -> str:
    """Send a conversation to the model and return its reply as text.

    `messages` is a list of `{"role": ..., "content": ...}` dicts. The model has no
    memory between calls, so pass the whole conversation each time.
    """
    ...

async def ask_oracle(question: str) -> str:
    """Put one yes/no question to the player who knows the secret object.

    Returns `"yes"` or `"no"`. A guess is a question of the form `"Is it a ...?"`.
    """
    ...
'''

REFERENCE = """
messages = [
    {
        'role': 'system',
        'content': (
            'You are guessing a secret object with yes/no questions. Reply with exactly one '
            'question. Start broad and narrow down. When confident, guess with "Is it a <object>?".'
        ),
    },
    {'role': 'user', 'content': 'start'},
]

questions = 0
answer = None
while questions < 20:
    question = await call_llm(messages)
    questions = questions + 1
    reply = await ask_oracle(question)
    messages.append({'role': 'assistant', 'content': question})
    messages.append({'role': 'user', 'content': reply})
    lowered = question.strip().lower()
    if lowered.startswith('is it a ') and reply == 'yes':
        answer = lowered[len('is it a ') :].rstrip('?').strip()
        break

{'answer': answer, 'questions': questions}
"""

TASK = Task(
    name='twenty_questions',
    category='agents',
    prompt=(
        'Play twenty questions from code. Keep a `messages` list with a system message '
        'telling the model to reply with exactly one yes/no question, starting broad, and to '
        'guess with "Is it a <object>?" when confident. Each round: ask `call_llm(messages)` '
        'for the next question, put it to `ask_oracle`, and append both the question (as an '
        'assistant message) and the answer (as a user message). Stop when a guess of the form '
        '"Is it a ...?" is answered "yes", or after 20 questions. Return a dict with "answer" '
        '(the guessed object in lower case, or None) and "questions" (how many were asked).'
    ),
    stubs=STUBS,
    tools={'ask_oracle': ask_oracle},
    expected={'answer': SECRET, 'questions': len(_SCRIPT)},
    reference_solution=REFERENCE,
    traps=('growing message history', 'stop condition on the guess', 'question budget'),
    # One model call and one oracle call per question, strictly alternating.
    expected_external_calls=2 * len(_SCRIPT),
    expected_call_batches=2 * len(_SCRIPT),
    max_result_bytes=100,
    sub_model_stub=_stub_call_llm,
)
