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

# Several prompts, both modes, three attempts of each combination.
uv run --group evals python -m evals.harness.runner --all --prompt v1_current,v4_codemode --mode both --repeat 3 \
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

## Tasks

Each task has a `<name>.md` next to its module describing the request, the host functions and how it is scored.

### artifacts/svg_bar_chart

Draw a bar chart of revenue by region as SVG and write it to `/output/chart.svg`, a read-write mount.
A predicate parses the written file and checks the bar heights are proportional to the data; a rubric judges legibility.

### dates/schedule_conflicts

Fetch a day's meetings, list every overlapping pair, and total the busy hours with overlaps counted once.
Scored with `ApproxExpected` against the computed answer.

### numeric/expense_budget

Total each team member's Q3 travel expenses and look up a custom budget only for those over the standard one.
No Monty gap is on the natural path, so it is the control: a prompt that fails here is a bad prompt.

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

### schema/large_result_filter

Fetch 2,000 events and return the five critical ones with two fields each.
`max_result_bytes` is set, so the `result_size` assertion fails any solution that returns more than the answer.

### stateful/followup_reuse

Fetch all orders and return total revenue, then answer a follow-up about the top region in the same session.
The follow-up expects zero host calls, so re-fetching fails `follow_up_calls_as_expected`.

### text/log_parse

Parse 500 log lines with a regex, count errors per service, and list the five slowest requests.
Scored with `EqualsExpected`.

### text/markdown_report

Render regional revenue as a markdown table with padded, aligned columns.
Scored on the exact rendered string.

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
