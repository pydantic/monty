"""A tool set split between code and model: `send_summary` is not in the sandbox.

`fetch_sales` is a host function the code may call; `send_summary` is registered on
the model as an ordinary tool and does not appear in the sandbox stubs, so the model
has to compute the figure in code and then call the tool itself with the text
(Anthropic's `allowed_callers` split). The dry run replays that call from
`reference_model_tool_calls`.
"""

from __future__ import annotations

import asyncio

from evals.harness.evaluators import ApproxExpected, Predicate
from evals.harness.task import Task

_SALES = {
    'EMEA': [1200.5, 940.75, 2050.0],
    'AMER': [2300.0, 3105.0],
    'APAC': [1875.25, 615.5],
}
REGIONS = sorted(_SALES)
TOTAL = round(sum(sum(v) for v in _SALES.values()), 2)
SUMMARY = f'Q3 total: {TOTAL:,.2f} across {len(REGIONS)} regions'

_sent: list[str] = []


def _reset() -> None:
    _sent.clear()


async def fetch_sales(region: str) -> list[float]:
    """Host: Q3 sale amounts for one region, with latency so gathered calls overlap."""
    await asyncio.sleep(0.005)
    return list(_SALES[region])


def send_summary(text: str) -> str:
    """Model-only tool: send the Q3 summary line to the finance channel.

    Args:
        text: the summary line, e.g. `Q3 total: 1,234.00 across 3 regions`.
    """
    _sent.append(text)
    return 'sent'


def _summary_sent(_result: object) -> bool:
    return _sent == [SUMMARY]


STUBS = '''
REGIONS: list[str] = []
"""The sales regions."""

async def fetch_sales(region: str) -> list[float]:
    """Return every Q3 sale amount for `region`."""
    ...
'''

REFERENCE = """
import asyncio

per_region = await asyncio.gather(*[fetch_sales(region) for region in REGIONS])
total = 0.0
for amounts in per_region:
    for amount in amounts:
        total = total + amount
total = round(total, 2)
print(f'Q3 total: {total:,.2f} across {len(REGIONS)} regions')
total
"""

TASK = Task(
    name='split_callers',
    category='agents',
    prompt=(
        'Compute the total Q3 sales across every region in REGIONS in code and return it as a '
        'float rounded to 2 decimal places. Then, outside the code, use the send_summary tool '
        'to send exactly this line: "Q3 total: <total with thousands separators and 2 decimal '
        'places> across <number of regions> regions". send_summary is not available inside the '
        'sandbox.'
    ),
    stubs=STUBS,
    tools={'fetch_sales': fetch_sales},
    inputs={'REGIONS': REGIONS},
    expected=TOTAL,
    evaluators=(ApproxExpected(), Predicate('send_summary was called once with the exact line', _summary_sent)),
    reference_solution=REFERENCE,
    traps=('calling a model-only tool from code', 'format spec with thousands separator'),
    expected_external_calls=len(REGIONS),
    expected_call_batches=1,
    model_tools={'send_summary': send_summary},
    reference_model_tool_calls=(('send_summary', {'text': SUMMARY}),),
    setup=_reset,
)
