"""Sandboxed analysis code that runs inside Monty.

This code is executed in the Monty sandbox with access to external functions
for SQL queries, JSON parsing, and sentiment analysis.
"""

from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from type_stubs import analyze_sentiment, query_csv, read_json


async def main():
    # Step 1: Query top 10 customers by total purchases
    print('getting top customers...')
    top_customers = await query_csv(
        filepath=Path('/data/customers/customers.csv'),
        sql="""
        SELECT "First", "Last", "Email", "Total Purchased" as TotalPurchased
        FROM data
        ORDER BY "Total Purchased"
        DESC LIMIT 10
        """,
    )

    # Step 2: Get their Twitter handles from the survey data
    emails: list[str] = [c['Email'] for c in top_customers]
    print('getting twitter handles...')
    twitter_handles = await query_csv(
        Path('/data/customers/surveys.csv'),
        f"""
        SELECT "Email", "Twitter Username" as Twitter
        FROM data
        WHERE "Email" IN $emails
        """,
        parameters={'emails': emails},
    )
    email_to_twitter = {row['Email']: row['Twitter'] for row in twitter_handles}

    # Step 3: Load all tweets
    tweets = await read_json(filepath=Path('/data/tweets/tweets.json'))
    assert isinstance(tweets, list)

    print(f'processing {len(top_customers)} customers...')

    # Step 4: For each customer, find their tweets and analyze sentiment
    results: list[dict[str, object]] = []
    for customer in top_customers:
        twitter = email_to_twitter.get(customer['Email'])
        if not twitter:
            continue

        # Find tweets by this user
        user_tweets = [t for t in tweets if t['user'] == twitter]
        if not user_tweets:
            continue

        # Analyze sentiment of each tweet
        sentiments: list[float] = []
        for tweet in user_tweets:
            score = await analyze_sentiment(text=tweet['text'])
            sentiments.append(score)

        # Calculate average sentiment
        avg_sentiment = sum(sentiments) / len(sentiments)
        print(f'{customer["First"]} {customer["Last"]} - {avg_sentiment=}')

        results.append(
            {
                'name': f'{customer["First"]} {customer["Last"]}',
                'total_purchases': customer['TotalPurchased'],
                'twitter': twitter,
                'tweet_count': len(user_tweets),
                'avg_sentiment': round(avg_sentiment, 2),
            }
        )
        return results


# Return the analysis results
await main()  # pyright: ignore
