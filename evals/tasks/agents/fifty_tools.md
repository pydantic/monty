# fifty_tools

A fake SaaS API of fifty host functions across CRM, tickets, calendar, billing and files, the Code Mode shape.
Every function is generated from one spec, and the `.pyi` stubs the model sees are rendered from the same spec, so
the catalogue reads like an MCP server's.
The task, handling an email about invoice INV-2026-118, needs six of them in an order only the data reveals: find the
contact, the company, the open tickets, the invoice, add a note, book a follow-up.

Notes and events are numbered from counters that `setup` resets, so their ids repeat across attempts.

Scored with `ApproxExpected` on the summary dict, `result_size` (250 bytes) and exactly six host calls.
