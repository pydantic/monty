# pypi_downloads

The Py AI March 2026 demo: sixty days of per-installer download counts in a `downloads` table, with one day where a
`bandersnatch` mirror sync adds forty thousand.
The question is which day spiked and whether it was real usage, a mirror or CI, with the share of that day's
downloads the cause accounts for.
Host functions: `sql_query` (sync), `plot` (async, recorded through `ChartRecorder`) and `display_table` (sync,
recorded).

Scored with `ApproxExpected` on `{'spike_date', 'cause', 'share'}`, a `Predicate` that a daily plot of at least
thirty points and one table were shown, and `result_size`.
