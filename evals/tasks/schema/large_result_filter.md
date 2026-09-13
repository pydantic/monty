# large_result_filter

Call `fetch_events()` for about 2,000 events and return the ones with severity `critical` as `id`/`service` dicts sorted
by id.
Exactly five events are critical.

Scored with `EqualsExpected`, plus the `result_size` assertion from `max_result_bytes=400`, so a solution that returns
every event fails even though the answer is inside it.
One host call is expected.
