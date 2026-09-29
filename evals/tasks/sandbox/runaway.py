"""A list that never stops growing, then a follow-up in the same session.

The primary request cannot succeed: it asks for more memory than the session's
`max_memory`, so the pass condition is a `MemoryError` raised in the sandbox. The
follow-up then proves the session is still usable once the global holding the
memory is reassigned. A time limit is different: `max_duration_secs` is a budget for
the whole session, so a `TimeoutError` ends everything after it (see the doc).
"""

from __future__ import annotations

from evals.harness.task import Task, Turn
from pydantic_monty import ResourceLimits

STUBS = '''
"""No host functions. The session has a 30 MB memory limit."""
'''

REFERENCE = """
chunks = []
while len(chunks) < 1000000:
    chunks.append('x' * 10000)
len(chunks)
"""

FOLLOW_UP_REFERENCE = """
chunks = None
total = 0
for i in range(1, 1001):
    total = total + i * i
total
"""

TASK = Task(
    name='runaway',
    category='sandbox',
    prompt=(
        'Append the string "x" * 10000 to a list called `chunks` until it holds 1,000,000 '
        'items, then return the length of the list.'
    ),
    stubs=STUBS,
    tools={},
    limits=ResourceLimits(max_memory=30_000_000),
    reference_solution=REFERENCE,
    traps=('MemoryError is not catchable', 'globals keep their memory after a failed feed'),
    expected_external_calls=0,
    expect_error='MemoryError',
    follow_up=Turn(
        prompt=(
            'That ran out of memory. Release the list by rebinding `chunks` to None, then '
            'return the sum of the squares of the integers from 1 to 1000 inclusive.'
        ),
        expected=333833500,
        reference_solution=FOLLOW_UP_REFERENCE,
        expected_external_calls=0,
    ),
)
