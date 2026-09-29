# world_cup

The `pydantic/talks` workshop shape: football analytics over a small World Cup SQLite database.
`matches`, `goals` and `teams` spell team names one way, `events` and `event_matches` another, and `team_meta`
maps between them.
`goals.minute` is text that can read `'90+3'`; `events.qualifiers` is JSON, with `BigChance` marking a big chance.

The question asks, per Group A team, for big chances, goals, late goals (minute 80 or later) and conversion, a bar
chart of big chances via `draw_chart`, an SVG of conversion written to the read-write `/output` mount, and a markdown
table with padding rules spelled out in the prompt.
Host functions: `query`, `list_tables`, `describe_table` (read-only, sync) and `draw_chart` (async).

Scored with `EqualsExpected` on `{'best', 'table'}`, so the table's whitespace has to be exact, plus a `Predicate`
that reads the recorded chart call and parses the SVG's `<rect>` heights.
