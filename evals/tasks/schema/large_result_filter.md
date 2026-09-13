# large_result_filter

Call `fetch_events()` for about 2,000 events and return the ones with severity `critical` as `id`/`service` dicts sorted
by id.
Exactly five events are critical.

Scored with `Every` of an `Exact` match and a predicate that the result's `repr` is under 400 bytes.
`max_result_bytes=400` is also set on the task, so a solution that returns every event fails at the executor.
One host call is expected.
