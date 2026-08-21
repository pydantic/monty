"""Summary statistics with no `statistics` module.

Every part of this is a few lines of arithmetic, which is exactly why it is worth
scoring: the model has to notice the module is missing and write the arithmetic rather
than import it. The percentile is the interesting one — there are several defensible
definitions, so the prompt pins linear interpolation between ranks to keep the expected
value unambiguous.
"""

from __future__ import annotations

from evals.harness.task import Approx, Task

_LATENCIES = [
    12.4, 15.1, 9.8, 22.3, 18.7, 11.2, 45.9, 13.6, 17.0, 20.1,
    8.5, 31.2, 14.8, 16.3, 19.9, 27.4, 10.7, 23.8, 12.9, 35.6,
    13.1, 21.5, 11.9, 25.0, 16.8, 14.2, 29.3, 18.1, 10.3, 24.7,
]  # fmt: skip


async def fetch_latencies() -> list[float]:
    """Host function: the raw request latencies in milliseconds."""
    return list(_LATENCIES)


STUBS = '''
async def fetch_latencies() -> list[float]:
    """Return every recorded request latency, in milliseconds."""
    ...
'''


def _expected() -> dict[str, float]:
    values = sorted(_LATENCIES)
    count = len(values)
    total = sum(values)
    mean = total / count
    middle = count // 2
    median = values[middle] if count % 2 else (values[middle - 1] + values[middle]) / 2
    variance = sum((x - mean) ** 2 for x in values) / (count - 1)
    rank = 0.9 * (count - 1)
    low = int(rank)
    high = min(low + 1, count - 1)
    p90 = values[low] + (rank - low) * (values[high] - values[low])
    return {
        'count': count,
        'mean': round(mean, 3),
        'median': round(median, 3),
        'p90': round(p90, 3),
        'stdev': round(variance**0.5, 3),
    }


REFERENCE = """
values = sorted(await fetch_latencies())
count = len(values)

total = 0.0
for value in values:
    total = total + value
mean = total / count

middle = count // 2
if count % 2 == 1:
    median = values[middle]
else:
    median = (values[middle - 1] + values[middle]) / 2

squared = 0.0
for value in values:
    squared = squared + (value - mean) ** 2
stdev = (squared / (count - 1)) ** 0.5

rank = 0.9 * (count - 1)
low = int(rank)
high = low + 1
if high > count - 1:
    high = count - 1
p90 = values[low] + (rank - low) * (values[high] - values[low])

{
    'count': count,
    'mean': round(mean, 3),
    'median': round(median, 3),
    'p90': round(p90, 3),
    'stdev': round(stdev, 3),
}
"""

TASK = Task(
    name='stats_summary',
    category='numeric',
    prompt=(
        'Summarise the request latencies. Return a dict with "count", "mean", "median", '
        '"p90" and "stdev". Use the sample standard deviation (divide by n-1). For p90 '
        'use linear interpolation between the two ranks either side of 0.9*(n-1). Round '
        'every value except "count" to 3 decimal places.'
    ),
    stubs=STUBS,
    tools={'fetch_latencies': fetch_latencies},
    expected=Approx(_expected()),
    reference_solution=REFERENCE,
    traps=('statistics module', 'math.fsum', 'math.prod'),
    expected_external_calls=1,
)
