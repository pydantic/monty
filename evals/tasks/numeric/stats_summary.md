# stats_summary

Call `fetch_latencies()` for 30 request latencies in milliseconds and return `count`, `mean`, `median`, `p90` and
`stdev`, all but `count` rounded to three decimal places.

The prompt fixes the definitions: sample standard deviation (divide by n-1), and p90 by linear interpolation between the
ranks either side of `0.9 * (n - 1)`.

Scored with `ApproxExpected`; one host call is expected.
