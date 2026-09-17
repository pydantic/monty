# calendar_fill

Create a 30-minute "Daily stand-up" at 08:00 local time on every weekday of October 2026 (the month after `TODAY`)
in London, New York and Sydney, skipping any day that already has an appointment at that instant.
October 2026 has a DST change in Sydney on the 4th and in London on the 25th.

Host functions: `utc_offset_hours(region, day)` supplies the offset per date since Monty has no `zoneinfo`,
`get_appointments(region, start, end)` returns existing entries as ISO-8601 strings with offsets, and
`create_appointment(region, start, duration_minutes, title)` takes a timezone-aware `datetime` across the boundary.
Three seeded 08:00 appointments must be skipped; a 09:00 one must not.

Scored with `EqualsExpected` on `{"created": 63}` and a `Predicate` that exactly the expected UTC instants were
booked, each once, with the right title and duration.
`setup` restores the seeded calendars.
