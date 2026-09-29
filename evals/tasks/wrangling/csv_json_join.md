# csv_json_join

Call `read_customers_csv()` for a five-row CSV (`handle`, `name`, `company`, `total_spend`) and `read_tweets_json()` for
a JSON list of tweets with `handle`, `text` and `sentiment`.
Join them on `handle` and return the three customers with the highest spend as `name`/`total_spend`/`avg_sentiment`
dicts, sentiment rounded to three decimal places.

Two company names contain a comma inside double quotes, so splitting lines on `,` shifts the spend column and produces a
wrong answer rather than an error.

Scored with `ApproxExpected`; two host calls are expected.
