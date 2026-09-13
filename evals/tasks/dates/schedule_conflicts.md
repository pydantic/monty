# schedule_conflicts

Call `fetch_meetings()` for seven meetings on one day, each with `title`, `start` and `end` as ISO-8601 strings.
Return `{"conflicts": [...], "busy_hours": float}`: every overlapping pair as a sorted `[title_a, title_b]` list, the
pairs themselves sorted, and the hours covered by at least one meeting with overlaps counted once, rounded to two
decimal places.

Scored with `ApproxExpected` against a value computed from the same fixture.
One host call is expected.

The reference solution stores each meeting as a tuple rather than a dict.
A dict holding a title and two datetimes types as `dict[str, str | datetime]`, and the type checker rejects subtracting
two of its values before the code runs.
