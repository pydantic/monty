"""A ReAct-style agent loop written in the sandbox around `call_llm`.

The model is asked to write the loop, not to be it: sandboxed code keeps the message
history, calls `call_llm`, parses the JSON step it replies with, dispatches tool
calls to host functions, and stops on a final answer. `_stub_call_llm` plays the
model under `--dry-run`, choosing the next step from how many tool results the
transcript already holds.
"""

from __future__ import annotations

import json
from typing import Any

from evals.harness.evaluators import ApproxExpected
from evals.harness.task import Task

_ORDERS: dict[str, dict[str, Any]] = {
    'O-1001': {'order_id': 'O-1001', 'customer_id': 'C-42', 'total': 85.0, 'items': ['kettle', 'mug']},
    'O-1002': {'order_id': 'O-1002', 'customer_id': 'C-7', 'total': 19.5, 'items': ['spoon']},
}
_CUSTOMERS: dict[str, dict[str, Any]] = {
    'C-42': {'customer_id': 'C-42', 'name': 'Ada Lovelace', 'tier': 'gold'},
    'C-7': {'customer_id': 'C-7', 'name': 'Alan Turing', 'tier': 'silver'},
}
_REFUND_RATE = {'damaged': 0.5, 'late': 0.2}


async def lookup_order(order_id: str) -> dict[str, Any]:
    """Host tool: one order by id."""
    return dict(_ORDERS[order_id])


async def lookup_customer(customer_id: str) -> dict[str, Any]:
    """Host tool: one customer by id."""
    return dict(_CUSTOMERS[customer_id])


async def compute_refund(order_id: str, reason: str) -> dict[str, Any]:
    """Host tool: the refund policy applied to an order; gold customers are always approved."""
    order = _ORDERS[order_id]
    customer = _CUSTOMERS[order['customer_id']]
    amount = round(order['total'] * _REFUND_RATE.get(reason, 0.0), 2)
    return {'amount': amount, 'approved': customer['tier'] == 'gold' or amount < 20}


def _stub_call_llm(prompt: str) -> str:
    """Scripted model: the step depends on how many `tool:` results the transcript holds."""
    tool_lines = [line for line in prompt.split('\n') if line.startswith('tool: ')]
    if len(tool_lines) == 0:
        return json.dumps({'tool': 'lookup_order', 'args': {'order_id': 'O-1001'}})
    if len(tool_lines) == 1:
        order = json.loads(tool_lines[0][len('tool: ') :])
        return json.dumps({'tool': 'lookup_customer', 'args': {'customer_id': order['customer_id']}})
    if len(tool_lines) == 2:
        return json.dumps({'tool': 'compute_refund', 'args': {'order_id': 'O-1001', 'reason': 'damaged'}})
    order = json.loads(tool_lines[0][len('tool: ') :])
    customer = json.loads(tool_lines[1][len('tool: ') :])
    refund = json.loads(tool_lines[2][len('tool: ') :])
    return json.dumps(
        {
            'final': {
                'order_id': order['order_id'],
                'customer_name': customer['name'],
                'refund': refund['amount'],
                'approved': refund['approved'],
            }
        }
    )


STUBS = '''
from typing import Any

async def call_llm(messages: list[dict[str, str]]) -> str:
    """Send a conversation to the model and return its reply as text.

    `messages` is a list of `{"role": ..., "content": ...}` dicts with roles
    `system`, `user`, `assistant` or `tool`. The model has no memory between calls.
    """
    ...

async def lookup_order(order_id: str) -> dict[str, Any]:
    """Return an order: `order_id`, `customer_id`, `total`, `items`."""
    ...

async def lookup_customer(customer_id: str) -> dict[str, Any]:
    """Return a customer: `customer_id`, `name`, `tier`."""
    ...

async def compute_refund(order_id: str, reason: str) -> dict[str, Any]:
    """Apply the refund policy: returns `amount` and `approved`."""
    ...
'''

REFERENCE = """
import json

SYSTEM = (
    'You are a support agent. Reply with exactly one JSON object per turn: either '
    '{"tool": <name>, "args": {...}} to call lookup_order, lookup_customer or '
    'compute_refund, or {"final": {...}} when done. The final object must have keys '
    'order_id, customer_name, refund and approved.'
)

messages = [
    {'role': 'system', 'content': SYSTEM},
    {'role': 'user', 'content': 'Customer C-42 wants a refund on order O-1001: the kettle arrived damaged.'},
]

final = None
for _ in range(8):
    reply = await call_llm(messages)
    messages.append({'role': 'assistant', 'content': reply})
    try:
        step = json.loads(reply)
    except ValueError:
        messages.append({'role': 'user', 'content': 'That was not valid JSON. Reply with one JSON object.'})
        continue
    if 'final' in step:
        final = step['final']
        break
    name = step['tool']
    args = step['args']
    if name == 'lookup_order':
        result = await lookup_order(args['order_id'])
    elif name == 'lookup_customer':
        result = await lookup_customer(args['customer_id'])
    elif name == 'compute_refund':
        result = await compute_refund(args['order_id'], args['reason'])
    else:
        result = {'error': f'unknown tool {name}'}
    messages.append({'role': 'tool', 'content': json.dumps(result)})

final
"""

TASK = Task(
    name='agent_loop',
    category='agents',
    prompt=(
        'Implement a support agent as a loop in code. Keep a `messages` list starting with a '
        'system message that tells the model to reply with exactly one JSON object per turn: '
        '{"tool": name, "args": {...}} to call one of the tools, or {"final": {...}} when done. '
        'Each turn: call `call_llm(messages)`, append its reply as an assistant message, parse '
        'the JSON, run the named tool, and append the result as a `tool` message. If the reply '
        'is not valid JSON, tell the model so and continue. Stop after at most 8 turns. The '
        'request to handle: customer C-42 wants a refund on order O-1001 because the kettle '
        'arrived damaged. Return the final object, which must have keys "order_id", '
        '"customer_name", "refund" and "approved".'
    ),
    stubs=STUBS,
    tools={'lookup_order': lookup_order, 'lookup_customer': lookup_customer, 'compute_refund': compute_refund},
    expected={'order_id': 'O-1001', 'customer_name': 'Ada Lovelace', 'refund': 42.5, 'approved': True},
    evaluators=(ApproxExpected(),),
    reference_solution=REFERENCE,
    traps=('json.loads on model text', 'dispatch by name without eval', 'loop cap'),
    # Four model turns and three tool calls, strictly sequential.
    expected_external_calls=7,
    expected_call_batches=7,
    max_result_bytes=200,
    sub_model_stub=_stub_call_llm,
)
