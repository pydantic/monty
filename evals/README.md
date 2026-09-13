# Agent-code evals

Tasks an agent solves by writing Python for Monty, a [pydantic-evals](https://pydantic.dev/docs/ai/evals/evals/) harness
that runs a model against them under a chosen system prompt, and reports on the results.

The suite has three uses:

- Compare system prompts: which prompt text gets code Monty runs first time, and at what token cost.
- Rank missing features: which CPython constructs and modules block generated code most often.
- Compare models: how well each model writes Monty code under the same prompt.

## Running

```bash
make dev-py   # builds the worker the harness runs code in

# Run every task's reference solution through Monty; no model, no API key.
uv run --group evals python -m evals.harness.runner --all --dry-run

# One task, one prompt, one model.
uv run --group evals python -m evals.harness.runner --task numeric/expense_budget --prompt v4_codemode --model anthropic:claude-sonnet-4-5

# Several prompts, every mode, three attempts of each combination.
uv run --group evals python -m evals.harness.runner --all --prompt v1_current,v4_codemode --mode all --repeat 3 \
    --model anthropic:claude-sonnet-4-5 --judge-model anthropic:claude-sonnet-4-5
```

Each prompt × model × mode combination is one pydantic-evals experiment, printed as a table when it finishes.
`evals/reports/` (gitignored) then gets `scoreboard.md` and `scoreboard.json`, the per-case metrics for every experiment
side by side, and `feature_gaps.md`, the Monty gaps hit, by prompt.
Cases run one at a time unless `--concurrency` is raised; repeats of a task share its stateful tools and mount, so they
must not overlap.

`--dry-run` executes each task's `reference_solution` instead of calling a model.
A reference solution that fails means the task is wrong or Monty has a gap; fix one before using the task to score a
model.

## How it works

A task (`evals/tasks/<category>/<name>.py`) exports a `Task`: the request, `.pyi` stubs for the host functions the model
may call, those host functions, an expectation for the result, and a reference solution.
The runner substitutes the stubs into the prompt variant, sends the request, extracts the code block, runs it in a Monty
session with the host functions attached, and checks the trailing expression against the expectation.

Two modes:

- `single`: one code block, executed, scored.
    Measures the prompt.
- `agentic`: Monty's error and printed output go back to the model as the next turn, up to `--max-turns` (default 4).
    Measures what error feedback repairs.

The session stays open for the whole attempt, so a task's `follow_up` turn can check that the model reuses globals
instead of re-fetching.

A task with a `sub_model_stub` also gets `llm_query(prompt)` and `call_llm(messages)` host functions.
With a model each is one plain completion of that model (`call_llm` renders its messages as `role: content` lines), and
its tokens count towards the attempt; under `--dry-run` the task's stub answers instead.
Batching is the sandboxed code's job, so gathered sub-calls show up in `call_batches` like any other host call.

Three more task fields change how a run is driven.
`expect_error` names the exception the primary request must end with, so a resource-limit case passes on that error and
its follow-up still runs.
`model_tools` are tools only the model may call, registered on the pydantic-ai agent and absent from the sandbox stubs;
a dry run replays `reference_model_tool_calls` in their place.
`snapshot_at` names a sync host function at whose call the executor dumps the suspended run, discards the session,
restores the dump in a fresh one and answers the call there; the `snapshots` metric counts it.

Evaluators (`evaluators.py`) turn that into assertions, one column per objective axis rather than one number:

| Axis        | Assertions and metrics                                                                   |
| ----------- | ---------------------------------------------------------------------------------------- |
| Correctness | the task's own evaluator, `first_attempt_runs`, `type_check_passed`, `calls_as_expected` |
| Cost        | `prompt_tokens` + `completion_tokens`, `turns`, `result_bytes` / `result_size`           |
| Time        | `call_batches`, the number of sequential waves of host calls, and `within_call_budget`   |
| Simplicity  | `code_lines`, `max_nesting`, and `LLMJudge` where a task has a rubric                    |

The task's own evaluator is `EqualsExpected`, `ApproxExpected` (a float tolerance, for anything with arithmetic in it),
`Predicate` (a host-side check, for answers no literal can express) or `LLMJudge`.
The dataset-level evaluators read the task's pinned expectations and skip themselves when a task pins nothing.
A case passes when every assertion holds.

`call_batches` counts overlapping host calls as one wave: twelve calls under `asyncio.gather` score 1, twelve awaited in
a loop score 12.
A host function that never awaits completes before the next one starts, so a task that scores `expected_call_batches`
must give its host functions an `await asyncio.sleep(...)` (see `HOST_LATENCY` in `orchestration/weather_fanout.py`).

`classify.py` maps each failure to a Monty feature gap when the error names a symbol in the `gaps.py` tables, and to a
model mistake otherwise.
`feature_gaps.md` groups gaps by the prompt they were hit under: a gap only weak prompts hit is a prompt problem, a gap
the best prompt still hits is a feature to build.

Sessions run with `type_check=True`, so the bundled type checker runs before execution; its failures fail the
`type_check_passed` assertion and are recorded as `type_check` gaps.

`--judge-model` sets the model for `LLMJudge` evaluators; without it they are dropped from the cases, so a dry run
scores only the machine-checkable parts of a rubric task.

## Prompt variants

`evals/prompts/<variant>.md`, selected by name with `--prompt`; `{stubs}` is replaced with the task's stubs.

| Variant       | Contents                                                                                  |
| ------------- | ----------------------------------------------------------------------------------------- |
| `v0_bare`     | Return one code block; the last expression is the result.                                 |
| `v1_current`  | The original `examples/web_scraper` prompt.                                               |
| `v2_accurate` | The available modules, plus lists of the modules, attributes and syntax that are missing. |
| `v3_idioms`   | The available modules, plus recipes for what to write instead of missing features.        |
| `v4_codemode` | v3 plus strategy: loop in code, `gather` independent calls, return only what is needed.   |
| `v5_minimal`  | v4's content in the fewest words.                                                         |

## Coverage

One row per case: what the agent is asked to do, and what Monty has to get right for it.
The workloads come from agent-code practice, the demos in `pydantic/talks`, and the literature: Anthropic's
programmatic tool calling, CodeAct, Cloudflare's Code Mode, smolagents and the Recursive Language Models paper.
Every case is built; a case whose reference solution fails on a Monty gap stays in the suite, since the failure is the
measurement.

| Case                  | Goal                                                                                                                                                                                              | Needs                                                                                                                                                                                                                                                             |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `weather_fanout`      | Fetch twelve cities' weather and return the three coldest in Celsius.                                                                                                                             | Async host functions and `asyncio.gather`, so twelve calls cost one round trip.                                                                                                                                                                                   |
| `paginate_and_count`  | Follow a cursor through every page of orders and count them by status.                                                                                                                            | Awaiting a host function in a loop whose length is only known as it runs.                                                                                                                                                                                         |
| `expense_budget`      | Total each engineer's travel expenses and flag who is over their own budget.                                                                                                                      | Awaiting host functions inside branches; the control task, nothing Monty lacks.                                                                                                                                                                                   |
| `retry_flaky`         | Fetch eight records from a flaky source, retrying each up to three times.                                                                                                                         | Host exceptions raised as Python exceptions, `try`/`except` around `await`.                                                                                                                                                                                       |
| `large_result_filter` | Pull 2,000 events and return only the five critical ones.                                                                                                                                         | A large value into the sandbox and filtering before return, so the result stays under 400 bytes.                                                                                                                                                                  |
| `followup_reuse`      | Total revenue, then answer "which region" without fetching again.                                                                                                                                 | Globals persisting between feeds of one session.                                                                                                                                                                                                                  |
| `csv_json_join`       | Join a quoted CSV to JSON tweets and rank customers by spend with mean sentiment.                                                                                                                 | CSV parsing with quoted fields (`csv` would do), `json`, a dict join.                                                                                                                                                                                             |
| `log_parse`           | Count errors per service and find the five slowest requests in 500 log lines.                                                                                                                     | `re` over multi-line text, sorting on a compound key.                                                                                                                                                                                                             |
| `group_by_report`     | Group sales by region then product with sums and counts.                                                                                                                                          | Nested dicts with `setdefault`, rounding (`itertools.groupby` is the reflex).                                                                                                                                                                                     |
| `stats_summary`       | Mean, median, p90 and sample standard deviation of 30 latencies.                                                                                                                                  | The arithmetic written out (`statistics` is the reflex).                                                                                                                                                                                                          |
| `schedule_conflicts`  | List overlapping meetings and total busy hours, overlaps counted once.                                                                                                                            | `datetime` parsing and comparison, interval merging, record shapes the type checker accepts.                                                                                                                                                                      |
| `markdown_report`     | Render regional revenue as an aligned markdown table.                                                                                                                                             | f-string width, alignment and thousands specs (`str.format` is the reflex).                                                                                                                                                                                       |
| `svg_bar_chart`       | Draw a bar chart as SVG and write it to a mounted directory.                                                                                                                                      | `pathlib` writes through a read-write mount, an `LLMJudge` for legibility.                                                                                                                                                                                        |
| `support_tickets`     | Answer a question over a 600 KB ticket log bound as a variable (the RLM case).                                                                                                                    | String and `re` work over a large input, batched `llm_query` sub-calls, `--mode repl` iteration.                                                                                                                                                                  |
| `agent_loop`          | Implement a ReAct-style agent in the sandbox: `call_llm` returns text or tool calls, the code dispatches them to host functions and loops until a final answer.                                   | A `call_llm(messages, tools)` host function, message history as lists of dicts, `json` parsing of tool calls, retries on malformed replies, an iteration cap.                                                                                                     |
| `twenty_questions`    | Play twenty questions from the sandbox: ask `call_llm` for the next question and `ask_oracle` for the answer until the object is guessed (the PyCon Italy demo).                                  | A loop with a stop condition and a question budget, history growing across `call_llm` calls, game state held in the sandbox.                                                                                                                                      |
| `nested_rlm`          | Answer over a 5 MB log by spawning child RLMs on chunks.                                                                                                                                          | A host `rlm_query` that starts a nested session, a depth limit, memory limits on large strings.                                                                                                                                                                   |
| `fifty_tools`         | Complete a workflow against fifty MCP tools exposed as generated stubs (Code Mode).                                                                                                               | Stubs generated from tool schemas, tool selection in code, credentials held by the host.                                                                                                                                                                          |
| `split_callers`       | Finish a task where some tools are callable only from code and others only by the model.                                                                                                          | One turn mixing host functions with model-level tool calls (`allowed_callers`).                                                                                                                                                                                   |
| `thrust`              | Fly the `pydantic/thrust` rocket from launch pad to landing pad, with that repo's Logfire `pilot-instructions` prompt.                                                                            | Hundreds of sequential `await update(move)` calls, host dataclasses via `ClassType`, a compute budget per tick.                                                                                                                                                   |
| `world_cup`           | Answer analytics questions over the 2026 World Cup SQLite database and draw the charts asked for (the `pydantic/talks` workshop example).                                                         | `query` and `describe_table` host functions, joins finished in Python across tables that spell team names differently, `json` on the `qualifiers` column, `draw_chart`, SVG written to `/output`, exactly whitespaced markdown tables.                            |
| `ecommerce`           | Answer a dozen business questions (segments, retention, churn, reorder priority, mis-billed orders) over an ecommerce database that also takes writes (the PyData London example).                | Schema discovery with `list_tables` and `describe_table`, SQL reads and `insert_rows` writes, recovery from `{"error": ...}` results when the prompt's schema is stale, `datetime` cohorts, a `success`/`result` output.                                          |
| `dashboard`           | Build a multi-panel report from a warehouse: several SQL queries, the transforms between them, and one chart per panel across line, stacked bar, heatmap, scatter, histogram and small multiples. | Many `draw_chart` calls with distinct kinds and options, pivoting and binning in Python, axes kept consistent across panels, images written to a mount and a markdown page linking them.                                                                          |
| `pypi_downloads`      | Investigate why downloads of a package spiked and decide whether it is real usage (the Py AI March demo).                                                                                         | `sql_query` over parquet via a DuckDB host function, `plot`, `show_plot` and `display_table`, hypotheses tested across REPL turns with state kept between them.                                                                                                   |
| `lighthouse`          | Audit how agent-ready a domain is and print a scored markdown report (the AgentCon example).                                                                                                      | Two waves of gathered `fetch` and `dns_lookup` calls with `re` and `json` parsing between them, a 100-point rubric computed in code, a markdown table.                                                                                                            |
| `price_scrape`        | Extract every model's prices from a provider's pricing page and record each through a validating host function (the February demo).                                                               | A BeautifulSoup-like host proxy (`Tag` dataclass, `find`, `select`), 100 KB of HTML kept out of context, `record_model_info` per row, last run's optimal code reused.                                                                                             |
| `calendar_fill`       | Create an 8 am local-time appointment on every weekday next month for a team in three regions, on a week with a DST change (the PyCon calendar agent, extended).                                  | `datetime` and `timedelta` loops, `date.weekday()`, `zoneinfo` or host-supplied offsets, `datetime` arguments crossing the host boundary, many writes through host functions.                                                                                     |
| `reverse_proxy`       | Serve HTTP requests the way a Cloudflare Worker does: route by path, rewrite headers, rate-limit by IP, cache GETs, and proxy the rest to an origin.                                              | A `fetch(request)` entrypoint called once per request with a `Request` host object, a `fetch_origin` host function, a KV-style host object for cache and counters, `Response` construction, sub-millisecond start per request or a snapshot restored per request. |
| `black_scholes`       | Price a book of options and back out implied volatility.                                                                                                                                          | `math.erf`, `exp`, `log`, `sqrt`, Newton iteration.                                                                                                                                                                                                               |
| `irr_ladder`          | NPV, IRR and an amortisation schedule for a set of loans.                                                                                                                                         | Root finding, compounding, money rounding (`decimal` would do).                                                                                                                                                                                                   |
| `haversine_routes`    | Assign deliveries to depots and order each route by distance.                                                                                                                                     | `radians`, `atan2`, `sin`, `cos`, greedy routing loops.                                                                                                                                                                                                           |
| `anomaly_scan`        | Flag anomalous days in a year of hourly metrics pulled from a host function in pages.                                                                                                             | Moving averages, z-scores, `erf` for p-values, windows kept over paged input with bounded memory.                                                                                                                                                                 |
| `shift_roster`        | Build a week's rota under availability and hour caps.                                                                                                                                             | `itertools` combinations, constraint checks in tight loops within the time limit.                                                                                                                                                                                 |
| `polars_proxy`        | Analyse a DataFrame through a host-proxied polars API with sync methods.                                                                                                                          | `ClassInstance` / `ClassType` policies, sync method calls, chaining, expressions built from host objects.                                                                                                                                                         |
| `crm_sync`            | Sync contacts between a CRM, a ticketing system and a calendar with no duplicates.                                                                                                                | Many distinct host functions, idempotency keys (`hashlib` would do), ordered writes.                                                                                                                                                                              |
| `expense_policy`      | Apply a written expense policy plus user-submitted plugin rules to a month of claims, with a reason per decision.                                                                                 | Rules as functions and classes defined in the sandbox, `@dataclass` records, plugin exceptions reported per claim.                                                                                                                                                |
| `ledger_match`        | Reconcile bank lines against invoices with amount and date tolerances.                                                                                                                            | Fuzzy matching, date windows, an unmatched report over thousands of rows.                                                                                                                                                                                         |
| `redact_pii`          | Extract fields from emails written in several scripts, redact personal data, and dedupe the names.                                                                                                | `re` with named groups, `unicodedata` normalisation and `casefold`, structured output.                                                                                                                                                                            |
| `quote_builder`       | Quote a multi-currency order with tiers, tax and FX.                                                                                                                                              | Money rounding (`decimal` would do), `datetime` effective dates.                                                                                                                                                                                                  |
| `approval_gate`       | Pause a refund run for a human decision mid-flight, restore it on another worker, and resume.                                                                                                     | Suspension at a host call via `feed_start`, `dump()` mid-feed, `load_snapshot` elsewhere, resuming with the decision.                                                                                                                                             |
| `uploaded_files`      | Analyse a directory of CSVs the user mounted read-only.                                                                                                                                           | Read-only and overlay mounts, `pathlib` listing and reads, `csv`.                                                                                                                                                                                                 |
| `escape_attempts`     | Prompt-injected code tries to reach the host filesystem, network and environment.                                                                                                                 | No escape by any route; an error in the sandbox, not a crash.                                                                                                                                                                                                     |
| `runaway`             | A loop that never ends and a list that never stops growing.                                                                                                                                       | `TimeoutError` and `MemoryError` raised in the sandbox, session kept alive.                                                                                                                                                                                       |

One reference solution fails today: `shift_roster` needs `itertools.combinations`.
Building the rest found four more divergences that the references work around, each recorded in the task's `.md`:
functions have no `__name__`, `PermissionError` is missing from the bundled type stubs, `open`, `eval` and
`__import__` are rejected by the type checker rather than the runtime, and `dict.setdefault` is typed `None | Any`.

## Tasks

Each task has a `<name>.md` next to its module describing the request, the host functions and how it is scored.

### agents/agent_loop

Implement a support agent as a loop: keep `messages`, call `call_llm`, parse its one-line JSON step, dispatch to `lookup_order`, `lookup_customer` or `compute_refund`, and repeat until `{"final": ...}`.
Scored with `ApproxExpected` on the final dict, a 200-byte result limit, and seven calls in seven round trips.

### agents/fifty_tools

Handle an invoice email against fifty host functions generated from one spec, of which six are needed.
Scored with `ApproxExpected` on the summary dict and exactly six host calls.

### agents/nested_rlm

Split a 2 MB log bound as `context` into chunks, ask `rlm_query` to count ERROR lines per code and host in each, then gather and merge.
Scored with `ApproxExpected` and a two-wave call budget; the child is a sub-model call over the chunk, not a child REPL.

### agents/split_callers

Total Q3 sales in code, then call the model-only `send_summary` tool with the exact line; the dry run replays that call.
Scored with `ApproxExpected`, a `Predicate` on what `send_summary` received, and a one-wave budget.

### agents/twenty_questions

Play twenty questions from code, asking `call_llm` for each question and `ask_oracle` for the answer until a guess is confirmed.
Scored with `EqualsExpected` on the answer and question count, with twelve strictly sequential calls.

### artifacts/svg_bar_chart

Draw a bar chart of revenue by region as SVG and write it to `/output/chart.svg`, a read-write mount.
A predicate parses the written file and checks the bar heights are proportional to the data; a rubric judges legibility.

### business/crm_sync

Reconcile eight people across a CRM, a ticketing system and a calendar so each exists once per system under a canonical email, without duplicates.
Scored with `EqualsExpected` on created and updated counts and a `Predicate` over the final state.

### business/expense_policy

Decide 40 claims against a written policy and three plugin functions pasted from the prompt, one of which raises on some claims.
Scored with `EqualsExpected` against decisions computed by the same functions host-side; functions have no `__name__` in Monty.

### business/ledger_match

Reconcile 60 bank lines against 55 invoices: exact amount within three days first, then within 1% when the invoice number appears in the reference.
Scored with `EqualsExpected` on matches and both unmatched lists.

### business/quote_builder

Quote a seven-line order in three currencies with tiered prices, tax rates effective by date and FX to USD, rounding half up to cents at every step.
Scored with `ApproxExpected` at cent tolerance; four host calls in one batch.

### dates/calendar_fill

Book an 8 am local stand-up on every weekday of October 2026 in three regions with two DST changes, skipping days already taken, using host-supplied UTC offsets.
Scored with `EqualsExpected` on the count and a `Predicate` on the exact UTC instants booked.

### dates/schedule_conflicts

Fetch a day's meetings, list every overlapping pair, and total the busy hours with overlaps counted once.
Scored with `ApproxExpected` against the computed answer.

### files/uploaded_files

Count rows and quoted fields in CSVs from a read-only mount, skipping a non-CSV file, and show that a write is refused.
Scored with `EqualsExpected`; `except PermissionError` fails the bundled type check, so the reference catches `OSError`.

### numeric/anomaly_scan

Page through a year of hourly metrics, total per day, and flag days whose seven-day z-score gives an `erfc` p-value under the threshold.
Scored with `EqualsExpected`; nine sequential calls and a 200-byte result limit.

### numeric/black_scholes

Price twelve European options with Black-Scholes, using `math.erf` for the normal CDF, and back out three implied volatilities by Newton iteration.
Scored with `ApproxExpected`; two host calls gathered into one batch.

### numeric/expense_budget

Total each team member's Q3 travel expenses and look up a custom budget only for those over the standard one.
No Monty gap is on the natural path, so it is the control: a prompt that fails here is a bad prompt.

### numeric/irr_ladder

Compute each loan's level payment, NPV and IRR by bisection, then a 360-row amortisation schedule rounded to cents at every step.
Scored with `ApproxExpected` on the NPVs, IRRs, payment, sampled schedule rows and total interest; `decimal` is absent.

### numeric/stats_summary

Fetch 30 latencies and compute count, mean, median, p90 and sample standard deviation.
The prompt pins the p90 definition to linear interpolation so the expected value is unambiguous.

### orchestration/paginate_and_count

Follow a `next_cursor` through five pages of orders and count orders per status.
The page count is unknown up front, so the loop cannot be unrolled into a fixed set of calls.

### orchestration/retry_flaky

Fetch eight records from a host function that fails permanently for two of them and transiently for one, retrying up to
three times.
The tool keeps per-record attempt counters, so giving up early changes the answer, not just the call count.

### orchestration/weather_fanout

Fetch the weather for twelve cities, convert to Celsius, and return the three coldest.
Scored on `call_batches` as well as the answer: `asyncio.gather` costs one wave, a loop costs twelve.

### planning/haversine_routes

Assign fifteen deliveries to the nearest of three depots by haversine and order each route nearest-neighbour from the depot.
Scored with `ApproxExpected` on the per-depot orders and total km.

### planning/shift_roster

Build a week's rota where every shift is fully staffed by available people with a senior and nobody exceeds the hour cap.
Scored with a feasibility `Predicate`; fails until Monty implements `itertools.combinations`.

### proxy/polars_proxy

Answer a top-two-regions question by chaining `filter`, `group_by(...).agg`, `sort` and `head` on a host-proxied polars-shaped DataFrame.
Scored with `EqualsExpected` against the same chain run host-side.

### rlm/support_tickets

Answer a question over a 600 KB support-ticket log bound as `context`, the Recursive Language Models case.
The ticket text never names its issue type, so the model must filter in Python and classify with gathered
`llm_query` sub-calls; a sequential loop fails the call budget.

### sandbox/approval_gate

Process fifteen refunds, routing the eight above a threshold through a sync `request_approval` at which the executor snapshots the run and resumes it in a fresh session.
Scored with `ApproxExpected` on the totals and nine host calls; the `snapshots` metric records the moves.

### sandbox/escape_attempts

Make ten escape attempts against files, the environment and a read-only mount, recording each as blocked, plus two controls that must succeed.
Scored with `EqualsExpected`; `open`, `eval` and `__import__` are refused by the type checker, so the runtime never sees them.

### sandbox/runaway

Grow a list past a 30 MB `max_memory` so the run ends in `MemoryError`, then in the same session rebind the global and return a sum.
Scored with `expected_error` on the primary and `follow_up_correct`; a `TimeoutError` would instead exhaust the session's time budget.

### schema/large_result_filter

Fetch 2,000 events and return the five critical ones with two fields each.
`max_result_bytes` is set, so the `result_size` assertion fails any solution that returns more than the answer.

### sims/thrust

Fly the `pydantic/thrust` rocket from launch pad to landing pad on a ported, seeded copy of the game, one `await update(move)` per 0.1 s of flight.
Scored with a `Predicate` that the final status is `landed` before `max_flight_time`; a flight is hundreds of sequential round trips.

### sql/dashboard

Draw six named panels (line, stacked bar, heatmap, scatter, histogram, pie) from a year of orders, each from its own query and transform, and write `/output/report.md` linking them.
Scored with `ApproxExpected` on totals plus a `Predicate` over the recorded chart kinds, point counts and data, and the report file.

### sql/ecommerce

Find the acquisition channel with the highest average spend per customer when the prompt's documented column name is stale, and record the answer in a `reports` table.
Scored with `ApproxExpected` on the success and result dict, a `Predicate` that reads the `reports` row back, and `result_size`.

### sql/pypi_downloads

Find the day downloads spiked and decide whether it was real usage, a mirror or CI, plotting daily totals and showing the installer breakdown.
Scored with `ApproxExpected` on the spike date, cause and share, and a `Predicate` that a plot and a table were shown.

### sql/world_cup

Compute big chances, goals, late goals and conversion for each Group A team from a database whose event and results tables spell team names differently, draw a bar chart, write an SVG to `/output`, and return a padded markdown table.
Scored with `EqualsExpected` on the table and best team, plus a `Predicate` over the recorded chart call and the SVG.

### stateful/followup_reuse

Fetch all orders and return total revenue, then answer a follow-up about the top region in the same session.
The follow-up expects zero host calls, so re-fetching fails `follow_up_calls_as_expected`.

### text/log_parse

Parse 500 log lines with a regex, count errors per service, and list the five slowest requests.
Scored with `EqualsExpected`.

### text/markdown_report

Render regional revenue as a markdown table with padded, aligned columns.
Scored on the exact rendered string.

### text/redact_pii

Parse senders from `Name <email>` headers in five scripts, redact phones, addresses and names, and dedupe senders after NFKC and casefold.
Scored with `EqualsExpected`.

### web/lighthouse

Audit a fixture site's agent-readiness with the AgentCon 14-check rubric, fetching known URLs in one gathered wave and discovered ones in a second.
Scored with `EqualsExpected` on the score and grade and a call budget of two waves.

### web/price_scrape

Extract every current model's prices from a 100 KB pricing page through a BeautifulSoup-like host proxy and record each via a validating host function, adapting last run's stale code.
Scored with a `Predicate` that the recorded models match the five current ones exactly.

### web/reverse_proxy

Act as the edge worker for thirty scripted requests: rate-limit per IP in KV, proxy `/api/*`, cache `/static/*` with `x-cache` headers, gate `/admin` on a token and 404 the rest.
Scored with `EqualsExpected` on the request count and a `Predicate` comparing every recorded response to the rules.

### wrangling/csv_json_join

Parse a CSV with quoted fields by hand, join it to a JSON document on a shared key, and report the top three customers
by spend with their mean tweet sentiment.
A naive `split(',')` gives a wrong number rather than an error.

### wrangling/group_by_report

Group sales rows by region then product, summing amount and units and counting rows.
Scored with `ApproxExpected` against the nested dict.

## Adding a task

Create `evals/tasks/<category>/<name>.py` exporting `TASK = Task(...)`, write `<name>.md` beside it, and make
`--dry-run` pass.
Set `expected` and pick the evaluator: `EqualsExpected` for one right answer, `ApproxExpected` where rounding order
legitimately varies, `Predicate` when the property has to be parsed out of the result, and `LLMJudge` only where nothing
can be checked mechanically.
Pin `expected_external_calls`, `expected_call_batches` or `max_result_bytes` and the dataset-level evaluators assert
them.
Host functions may be sync or async; give them latency if the task scores `call_batches`.
A tool that keeps state between calls needs `setup` to reset it before each attempt.
