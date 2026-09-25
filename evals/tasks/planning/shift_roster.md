# shift_roster

Call `fetch_staff()` for seven people (role, available days) and `fetch_shifts()` for fourteen shifts with a required
headcount.
Build a rota where every shift is fully staffed by available, distinct people including at least one senior, and nobody
exceeds `MAX_HOURS` at `SHIFT_HOURS` per shift.
Return shift id to the list of staff ids.

There is no unique answer, so a `Predicate` checks feasibility of whatever roster comes back.
The reference backtracks over `itertools.combinations` of eligible staff per shift, which Monty does not implement (see
`limitations/itertools.md`), so the case fails at type-check time (`unresolved-attribute`) until it does; the fixture is
checked solvable on import.

Two host calls are expected, in one batch.
