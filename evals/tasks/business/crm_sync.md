# crm_sync

Make eight people exist exactly once in each of a CRM, a ticketing system and a calendar, with emails stored
trimmed and lower-case.
The seeded systems disagree on who is present and spell the same email with different case and whitespace.

Nine host functions, three per system: `*_list()`, `*_create(name, email)` and `*_update(record_id, email)`.
`setup` resets the stores.

Scored with `EqualsExpected` on `{"created": 8, "updated": 8}` and a `Predicate` over the final state: each system
holds every canonical email once with the right name, so a duplicate create fails it whatever was returned.
