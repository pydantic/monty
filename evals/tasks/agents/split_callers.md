# split_callers

A tool set split between code and model, Anthropic's `allowed_callers` shape.
`fetch_sales(region)` is a host function the code may call; `send_summary(text)` is registered on the model as an
ordinary tool and is absent from the sandbox stubs.
The model must total Q3 sales across `REGIONS` in code, return the float, and then call `send_summary` itself with
the exact line "Q3 total: <total> across 3 regions".

Under `--dry-run` there is no model, so `reference_model_tool_calls` replays the `send_summary` call after the
reference code runs.

Scored with `ApproxExpected` on the total, a `Predicate` that `send_summary` received exactly the expected line once,
three host calls, and a call budget of one wave.
