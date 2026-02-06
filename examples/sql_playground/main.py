"""SQL Playground Example: Customer Sentiment Analysis with SQL and JSON."""

from __future__ import annotations

import asyncio
from pathlib import Path

from external_functions import ExternalFunctions

import pydantic_monty
from pydantic_monty import MemoryFile, OSAccess

# Path to the mafudge datasets repository (adjust if needed)
THIS_DIR = Path(__file__).parent
REPO_ROOT = THIS_DIR.parent.parent
MAFUDGE_DATASETS = (REPO_ROOT / '..' / 'mafudge_datasets').resolve()
assert MAFUDGE_DATASETS.is_dir(), f'mafudge_datasets directory not found at {MAFUDGE_DATASETS}. '
SANDBOX_CODE_PATH = THIS_DIR / 'sandbox_code.py'

TYPE_STUBS = (THIS_DIR / 'type_stubs.pyi').read_text()
SANDBOX_CODE = SANDBOX_CODE_PATH.read_text()

# Read file contents
customers_csv = (MAFUDGE_DATASETS / 'customers' / 'customers.csv').read_text()
surveys_csv = (MAFUDGE_DATASETS / 'customers' / 'surveys.csv').read_text()
tweets_json = (MAFUDGE_DATASETS / 'tweets' / 'tweets.json').read_text()

# Create virtual filesystem with mounted files
fs = OSAccess(
    [
        MemoryFile('/data/customers/customers.csv', content=customers_csv),
        MemoryFile('/data/customers/surveys.csv', content=surveys_csv),
        MemoryFile('/data/tweets/tweets.json', content=tweets_json),
    ]
)


async def main():
    """Run the customer sentiment analysis in the Monty sandbox.

    Returns:
        List of analysis results for top customers with sentiment scores.
    """
    # Set up the virtual filesystem with data files

    # Create external functions that can access the filesystem
    external_funcs = ExternalFunctions(fs)

    # Create the Monty runner with type checking enabled
    m = pydantic_monty.Monty(
        SANDBOX_CODE_PATH.read_text(),
        script_name='sql_playground.py',
        external_functions=['query_csv', 'read_json', 'analyze_sentiment'],
        type_check=True,
        type_check_stubs=TYPE_STUBS,
    )

    # Run the analysis with external functions and OS access
    results = await pydantic_monty.run_monty_async(
        m,
        external_functions={
            'query_csv': external_funcs.query_csv,
            'read_json': external_funcs.read_json,
            'analyze_sentiment': external_funcs.analyze_sentiment,
        },
        os=fs,
    )

    if not results:
        print('No results found. Check if customers have matching Twitter handles and tweets.')
    for r in results:
        sentiment_emoji = '😊' if r['avg_sentiment'] > 0 else '😐' if r['avg_sentiment'] == 0 else '😞'
        print(f'  {r["name"]}')
        print(f'    Purchases: ${r["total_purchases"]:,}')
        print(f'    Twitter: @{r["twitter"]}')
        print(f'    Tweets: {r["tweet_count"]}')
        print(f'    Sentiment: {r["avg_sentiment"]:+.2f} {sentiment_emoji}')
        print()


if __name__ == '__main__':
    asyncio.run(main())
