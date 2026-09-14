# weather_fanout

For each of the twelve names in `CITIES` (passed as an input), call `get_weather(city)`, convert `temp_f` to Celsius
rounded to one decimal place, and return the three coldest as `city`/`temp_c` dicts, coldest first.

`get_weather` sleeps for `HOST_LATENCY` (20 ms) before returning.
Without that sleep every call would complete before the next started and gathered calls could not be told from
sequential ones.

Scored with `ApproxExpected`.
Twelve host calls are expected and `expected_call_batches` is 1, which only `asyncio.gather` (or equivalent) achieves.
