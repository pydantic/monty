"""Answer a question over a support-ticket log too large to read: the RLM case.

The whole log is bound as `context`, the shape of the Recursive Language Models paper
(Zhang, Kraska, Khattab, 2025): the model inspects the variable with code, filters it,
and delegates the part that needs reading to `llm_query` sub-calls. The ticket text
never names its issue type, so grep cannot answer; the filter is cheap in Python but
the classification needs a model, and the fan-out has to be batched with `gather`.

`_stub_llm_query` stands in for the sub-model under `--dry-run`: the phrasings are a
closed set with known labels, so it answers any prompt that contains a ticket body.
"""

from __future__ import annotations

import math
import random
from datetime import date, timedelta

from evals.harness.evaluators import ApproxExpected
from evals.harness.task import Task

_PHRASINGS: dict[str, tuple[str, ...]] = {
    'outage': (
        'Nothing loads for anyone on our team, the dashboard has been unreachable since this morning.',
        'The API has been returning 503 for the last hour.',
        'Everything is down again, our whole office cannot get in.',
        'We are seeing a "service unavailable" page since 09:00, affecting all users.',
    ),
    'billing': (
        'We were charged twice for July.',
        'The invoice total does not match the plan we are on.',
        'Our card was declined but the plan should still be active.',
        'Please send receipts for the last three payments.',
    ),
    'login': (
        'I reset my password but the new one is rejected.',
        'Two-factor codes never arrive by SMS.',
        'SSO redirects back to the sign-in page in a loop.',
        'Locked out after too many attempts, need the account unlocked.',
    ),
    'data-export': (
        'The CSV download only contains the first 1,000 rows.',
        'Scheduled report emails stopped arriving on Monday.',
        'Dates in the downloaded file are shifted by one day.',
        'Need a full dump of our account data for an audit.',
    ),
    'performance': (
        'Search takes over 30 seconds for any query.',
        'Pages are noticeably slower since the last release.',
        'Bulk update of 500 records timed out twice.',
        'The reports page spins for minutes before rendering.',
    ),
}
_LABEL_OF = {phrase: label for label, phrases in _PHRASINGS.items() for phrase in phrases}

_CUSTOMERS = [
    'Acme Ltd',
    'Bletchley Park',
    'Compiler Works',
    'Dijkstra BV',
    'Erlang Systems',
    'Fermat Finance',
    'Gödel Analytics',
    'Hopper Inc',
    'Ising Labs',
    'Jacquard Textiles',
    'Kernighan & Co',
    'Lovelace Ltd',
]
_TIERS = ('gold', 'silver', 'bronze')
_TICKETS = 4000
_FIRST_DAY = date(2026, 6, 1)
_DAYS = 102  # June 1 to September 10


def _build(seed: int) -> tuple[str, list[tuple[str, str, str, str]]]:
    """The log text plus `(date, customer, tier, label)` per ticket, for the expected answer."""
    rng = random.Random(seed)
    lines: list[str] = []
    rows: list[tuple[str, str, str, str]] = []
    for index in range(_TICKETS):
        day = (_FIRST_DAY + timedelta(days=rng.randrange(_DAYS))).isoformat()
        customer = rng.choice(_CUSTOMERS)
        tier = rng.choice(_TIERS)
        label = rng.choice(list(_PHRASINGS))
        body = rng.choice(_PHRASINGS[label])
        lines.append(f'--- T-{index + 1:05d} | {day} | {customer} | {tier}')
        lines.append(body)
        rows.append((day, customer, tier, label))
    return '\n'.join(lines) + '\n', rows


def _answer(rows: list[tuple[str, str, str, str]]) -> dict[str, object] | None:
    """The expected dict, or `None` when either ranking is tied and the seed must change."""
    selected = [row for row in rows if row[2] == 'gold' and row[0].startswith('2026-08')]
    by_type: dict[str, int] = {}
    for _, _, _, label in selected:
        by_type[label] = by_type.get(label, 0) + 1
    ranked = sorted(by_type.items(), key=lambda item: item[1], reverse=True)
    if ranked[0][1] == ranked[1][1]:
        return None
    issue = ranked[0][0]
    by_customer: dict[str, int] = {}
    for _, customer, _, label in selected:
        if label == issue:
            by_customer[customer] = by_customer.get(customer, 0) + 1
    top = sorted(by_customer.items(), key=lambda item: item[1], reverse=True)
    if top[0][1] == top[1][1]:
        return None
    return {
        'issue_type': issue,
        'count': ranked[0][1],
        'top_customer': top[0][0],
        'top_customer_count': top[0][1],
        '_selected': len(selected),
    }


def _pick() -> tuple[str, dict[str, object], int]:
    """First seed whose fixture has an unambiguous answer."""
    for seed in range(20260821, 20260921):
        context, rows = _build(seed)
        answer = _answer(rows)
        if answer is not None:
            selected = int(answer.pop('_selected'))  # pyright: ignore[reportArgumentType]
            return context, answer, selected
    raise RuntimeError('no seed produced an unambiguous answer')


CONTEXT, EXPECTED, _SELECTED = _pick()


def _stub_llm_query(prompt: str) -> str:
    """Dry-run stand-in for the sub-model: label every known ticket body in the prompt.

    One body gives the label alone, several give one label per line in order of
    appearance, so both a per-ticket and a batched prompt get a usable reply.
    """
    found = sorted((prompt.find(phrase), label) for phrase, label in _LABEL_OF.items() if phrase in prompt)
    return '\n'.join(label for _, label in found) if found else 'unknown'


STUBS = '''
context: str
"""The complete support ticket log, June to September 2026. About 600 KB."""

async def llm_query(prompt: str) -> str:
    """Ask a language model one question and return its reply as plain text.

    Each call is a separate conversation with no memory; put everything the model
    needs in `prompt`. Keep prompts under a few thousand characters.
    """
    ...
'''

REFERENCE = """
import asyncio

lines = context.split('\\n')
tickets = []
i = 0
while i < len(lines):
    line = lines[i]
    if line.startswith('--- '):
        header = line[4:].split(' | ')
        body = lines[i + 1] if i + 1 < len(lines) else ''
        tickets.append({'id': header[0], 'date': header[1], 'customer': header[2], 'tier': header[3], 'body': body})
        i += 2
    else:
        i += 1

selected = [t for t in tickets if t['tier'] == 'gold' and t['date'].startswith('2026-08')]

def prompt_for(body):
    return (
        'Classify this support ticket as exactly one of: outage, billing, login, data-export, performance. '
        'Reply with the label only.\\n\\nTicket: ' + body
    )

labels = []
for start in range(0, len(selected), 20):
    chunk = selected[start:start + 20]
    labels = labels + list(await asyncio.gather(*[llm_query(prompt_for(t['body'])) for t in chunk]))

by_type = {}
for label in labels:
    key = label.strip().lower()
    by_type[key] = by_type.get(key, 0) + 1
issue = sorted(by_type, key=lambda k: by_type[k], reverse=True)[0]

by_customer = {}
for t, label in zip(selected, labels):
    if label.strip().lower() == issue:
        by_customer[t['customer']] = by_customer.get(t['customer'], 0) + 1
top = sorted(by_customer, key=lambda k: by_customer[k], reverse=True)[0]

{'issue_type': issue, 'count': by_type[issue], 'top_customer': top, 'top_customer_count': by_customer[top]}
"""

TASK = Task(
    name='support_tickets',
    category='rlm',
    prompt=(
        'The variable `context` holds the complete support ticket log for June to September '
        '2026: about 4,000 tickets and 600 KB, far too much to print or read. Inspect a small '
        'sample first, then work on it with code. Each ticket is a header line '
        '`--- <id> | <date> | <customer> | <tier>` followed by one line of text written by the '
        'customer. Considering only tickets from gold-tier customers opened in August 2026: '
        'which issue type is most common, and which customer raised the most tickets of that '
        'type? The issue types are outage, billing, login, data-export and performance. The '
        'text never states the type, so classify it with `llm_query`. Return a dict with '
        '"issue_type", "count", "top_customer" and "top_customer_count".'
    ),
    stubs=STUBS,
    tools={},
    inputs={'context': CONTEXT},
    expected=EXPECTED,
    evaluators=(ApproxExpected(),),
    reference_solution=REFERENCE,
    traps=('printing the whole context', 'sequential llm_query loop', 'grepping for label words'),
    # One sub-call per selected ticket, gathered twenty at a time, is the budget; a
    # sequential loop blows it and a single gather of everything beats it.
    expected_call_batches=math.ceil(_SELECTED / 20),
    max_result_bytes=200,
    sub_model_stub=_stub_llm_query,
)
