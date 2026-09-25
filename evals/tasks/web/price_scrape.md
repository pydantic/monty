# price_scrape

The February 2026 pricing demo: `html` is a 100 KB pricing page bound as an input, and the sandbox reads it only
through a BeautifulSoup-like proxy.
`beautiful_soup` returns a `Tag` host object (a `ClassInstance` with eager attributes); `find`, `find_all`,
`select` (tag, `.class`, `#id` and descendant selectors), `select_one`, `get` and `get_text` run on the host over
a tree built with `html.parser`.
Every model row is recorded with `record_model_info`, which validates against a pydantic `ModelInfo`.
`previous_code` carries last run's script, whose `table.price-table` selector no longer matches the page, so it
has to be adapted rather than replayed.

Scored with a `Predicate` that the recorded models equal the five current ones exactly (the deprecated row must be
skipped), and `result_size`.
