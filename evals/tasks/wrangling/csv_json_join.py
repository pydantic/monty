"""Join a CSV to a JSON document on a shared key, then aggregate.

Monty ships `json` but not `csv`, so the model has to split the text itself. That is a
small thing to get wrong in a big way: a naive `line.split(',')` breaks on the quoted
company name in the fixture, which changes the parsed spend and therefore the answer.
The task is scored on the aggregate, so a broken parse shows up as a wrong number
rather than an exception.
"""

from __future__ import annotations

import json

from evals.harness.task import Approx, Task

_CSV = """handle,name,company,total_spend
@ada,Ada Lovelace,"Analytical Engines, Ltd",4820.50
@grace,Grace Hopper,Compiler Works,7310.00
@alan,Alan Turing,"Bletchley, Park & Co",2145.25
@edsger,Edsger Dijkstra,Shortest Path BV,5590.75
@barbara,Barbara Liskov,Substitution Inc,6002.00
"""

_TWEETS = [
    {'handle': '@ada', 'text': 'love the new release', 'sentiment': 0.8},
    {'handle': '@ada', 'text': 'support was slow', 'sentiment': -0.4},
    {'handle': '@grace', 'text': 'flawless migration', 'sentiment': 0.9},
    {'handle': '@grace', 'text': 'docs are great', 'sentiment': 0.7},
    {'handle': '@grace', 'text': 'pricing is steep', 'sentiment': -0.2},
    {'handle': '@alan', 'text': 'it broke again', 'sentiment': -0.9},
    {'handle': '@edsger', 'text': 'elegant api', 'sentiment': 0.6},
    {'handle': '@barbara', 'text': 'solid abstraction', 'sentiment': 0.5},
    {'handle': '@barbara', 'text': 'minor bug', 'sentiment': -0.1},
]


async def read_customers_csv() -> str:
    """Host function: the customer table as raw CSV text."""
    return _CSV


async def read_tweets_json() -> str:
    """Host function: the tweet corpus as a raw JSON string."""
    return json.dumps(_TWEETS)


STUBS = '''
async def read_customers_csv() -> str:
    """Return the customer table as CSV text.

    Columns: `handle`, `name`, `company`, `total_spend`. Fields containing a comma are
    quoted with double quotes.
    """
    ...

async def read_tweets_json() -> str:
    """Return the tweet corpus as a JSON string.

    A list of objects with `handle`, `text` and `sentiment` (a float from -1 to 1).
    """
    ...
'''


def _expected() -> list[dict[str, object]]:
    """Top three customers by spend, each with the mean sentiment of their tweets."""
    spend = {
        '@grace': 7310.00,
        '@barbara': 6002.00,
        '@edsger': 5590.75,
    }
    names = {'@grace': 'Grace Hopper', '@barbara': 'Barbara Liskov', '@edsger': 'Edsger Dijkstra'}
    rows: list[dict[str, object]] = []
    for handle, total in spend.items():
        sentiments = [float(t['sentiment']) for t in _TWEETS if t['handle'] == handle]
        rows.append(
            {
                'name': names[handle],
                'total_spend': total,
                'avg_sentiment': round(sum(sentiments) / len(sentiments), 3),
            }
        )
    return rows


REFERENCE = """
import json

def split_csv_line(line):
    fields = []
    current = ''
    in_quotes = False
    for char in line:
        if char == '"':
            in_quotes = not in_quotes
        elif char == ',' and not in_quotes:
            fields.append(current)
            current = ''
        else:
            current = current + char
    fields.append(current)
    return fields

csv_text = await read_customers_csv()
tweets = json.loads(await read_tweets_json())

lines = [line for line in csv_text.splitlines() if line.strip()]
header = split_csv_line(lines[0])
customers = []
for line in lines[1:]:
    customers.append(dict(zip(header, split_csv_line(line))))

by_handle = {}
for tweet in tweets:
    by_handle.setdefault(tweet['handle'], []).append(tweet['sentiment'])

rows = []
for customer in customers:
    scores = by_handle.get(customer['handle'], [])
    rows.append({
        'name': customer['name'],
        'total_spend': float(customer['total_spend']),
        'avg_sentiment': round(sum(scores) / len(scores), 3) if scores else 0.0,
    })

sorted(rows, key=lambda r: r['total_spend'], reverse=True)[:3]
"""

TASK = Task(
    name='csv_json_join',
    category='wrangling',
    prompt=(
        'Join the customer CSV to the tweet corpus on the twitter handle. Return the '
        'three customers with the highest total spend, highest first, as a list of dicts '
        'with keys "name", "total_spend" (a float) and "avg_sentiment" (the mean '
        "sentiment of that customer's tweets, rounded to 3 decimal places)."
    ),
    stubs=STUBS,
    tools={'read_customers_csv': read_customers_csv, 'read_tweets_json': read_tweets_json},
    expected=Approx(_expected()),
    reference_solution=REFERENCE,
    traps=('csv module', 'statistics.mean', 'quoted CSV fields'),
    expected_external_calls=2,
)
