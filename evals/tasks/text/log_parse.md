# log_parse

Call `read_log()` for 500 lines of the form `2026-08-21T09:04:12Z ERROR [billing] req-0251 handled in 431ms` and return
`errors_by_service` (service to number of `ERROR` lines) and `slowest` (the five slowest requests as `request`/`ms`
dicts, ties broken by request id).

Scored with `EqualsExpected`; one host call is expected.
