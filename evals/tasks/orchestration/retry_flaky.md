# retry_flaky

Fetch every id in `RECORD_IDS` (1 to 8, passed as an input) with `fetch_record(record_id)`, retrying a failure up to
three attempts in total.
Return `{"fetched": int, "failed": [ids]}`.

`fetch_record` raises `ValueError` every time for records 3 and 7, and for the first two attempts on record 5.
It counts attempts per record, and `setup` clears the counters before each attempt of the task, so a solution that does
not retry gets `fetched: 5` instead of the expected `fetched: 6`.

Scored with `EqualsExpected` against `{'fetched': 6, 'failed': [3, 7]}`; 14 host calls are expected.
