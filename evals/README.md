# Agent-code task suite

Monty exists to run code written by agents, but nothing in this repo measured whether an
agent can actually write code Monty accepts. This directory does, and it answers two
questions with one artefact:

1. **What should the system prompt say?** Some of Monty's divergences from CPython can be
   steered around with prompt text and some cannot. Enumerating all of them costs tokens on
   every turn and risks priming the model toward the constructs being forbidden.
2. **Which missing features actually matter?** `functools`, `yield`, `match`, `str.format`,
   class inheritance and `itertools.groupby` are all absent. Ranking them by how often they
   block real agent code turns a guess into a roadmap.

The evidence that this was worth building: the only LLM-facing prompt in the repo before
this suite, in `examples/web_scraper/main.py`, tells the model that only `sys`, `typing` and
`asyncio` are available. Monty ships ten more modules. Nobody noticed because nothing scored it.

## Running it

```bash
make dev-py                                            # the harness needs the built worker

# No API key, no spend: run every task's reference solution through Monty.
uv run python -m evals.harness.runner --all --dry-run

# One task, one prompt, one model.
uv run python -m evals.harness.runner \
    --task numeric/expense_budget --prompt v4_codemode --model anthropic:claude-sonnet-4-5

# The real comparison.
uv run python -m evals.harness.runner --all \
    --prompt v1_current,v4_codemode --mode both --repeat 3 \
    --model anthropic:claude-sonnet-4-5 --judge-model anthropic:claude-sonnet-4-5
```

Reports land in `evals/reports/` (gitignored): `scoreboard.md`, `scoreboard.json` and
`feature_gaps.md`.

### `--dry-run` is the part that runs in CI

Every task ships a `reference_solution` — Monty code known to produce the right answer.
`--dry-run` executes that instead of calling a model, which proves the executor, the tool
counters, the mounts and the expectation comparison all work without an API key.

It also enforces something more important: **a task whose reference solution fails is either
a broken task or a real Monty gap**, and either way it must be resolved before that task is
allowed to score a model. Without this, a failing task is ambiguous between a bad task, a bad
model and a genuine feature gap.

The dry-run path deliberately does not import `pydantic_ai`. That import is done inside
`CodeAgent` and `judge_result` — the one exception to the repo's imports-at-the-top rule,
noted in those modules — so a broken LLM dependency cannot take the whole harness down with
it. That is not hypothetical: `pydantic_ai` does not currently import on this repo's Python
3.14 environment.

## Design

### Tasks are realistic, and chosen so the idiomatic solution crosses risky surface

Not synthetic feature probes. If a model would never reach for a construct, its absence does
not matter. So `wrangling/csv_json_join` needs a CSV parsed with no `csv` module,
`wrangling/group_by_report` needs a group-by with no `itertools.groupby`, and
`orchestration/retry_flaky` needs error handling with no custom exception classes.

Each task records its `traps` — the gaps its natural solution runs into. That field is
documentation, not enforcement: the point is that the trap is *reachable*, not that it must
be hit.

### Two modes, reported separately

- **single** — one code block, executed, graded. Low variance, so it measures what the
  *prompt* achieved.
- **agentic** — Monty's error and any printed output feed back as the next user turn, capped
  at 4. It measures what error feedback can repair.

A prompt that only wins in agentic mode is buying turns, not quality. Averaging the two would
hide that, so the scoreboard keeps them apart.

The suite holds **one session open for the whole of a task attempt**, so globals persist
between turns and `stateful/followup_reuse` can check that a follow-up reuses them. That is a
property of how the host drives the session, not of Monty: `examples/web_scraper` checks out a
fresh session per turn, so nothing persists there and its prompt says so correctly. A prompt
shipped with the library will have to state this conditionally, or state it per integration.

### There is no single best prompt

Callers optimise for different things, and the prompt text trades between them, so the
scoreboard has one column per axis rather than a blended score:

| Axis | Measured by |
| --- | --- |
| Correctness | `success`, `first_attempt_runs`, `type_check_passed` |
| Cost | `total_tokens`, `turns_used`, `result_bytes` |
| Time | `call_batches` — sequential waves of host calls |
| Simplicity | `code_lines`, `max_nesting`, and the judge where a rubric applies |

These conflict. Telling a model to `gather` everything wins time and loses simplicity; a
short prompt wins cost and loses first-attempt correctness. The weighting is the reader's
call, which is why the report does not make it for them.

### The round-trip metric needs host functions that actually take time

`call_batches` counts waves of overlapping host calls: twelve awaits in a loop score 12, the
same twelve under `asyncio.gather` score 1. It is a wall-clock proxy, and it has one sharp
edge worth knowing about.

A host function that returns without ever awaiting completes inside its own coroutine step,
so gathered calls never overlap and the counter reads as fully sequential *even when the code
is correct*. Measured against the built worker:

| Host function | Code | Calls | Batches | Wall |
| --- | --- | ---: | ---: | ---: |
| instant | `gather` | 4 | 4 | 4ms |
| instant | sequential | 4 | 4 | 4ms |
| 50ms | `gather` | 4 | **1** | 53ms |
| 50ms | sequential | 4 | **4** | 205ms |

So Monty really does run external calls concurrently under `gather`, and the metric really
does detect it — but only with latency in the fixture. **Any task scoring
`expected_call_batches` must give its host functions a real `await`**; see `HOST_LATENCY` in
`orchestration/weather_fanout.py`.

### The feature-gap ledger

`classify.py` maps a failure either to a `FeatureGap` or to `None`, and the `None` case is
the load-bearing one: `NameError: name 'functools' is not defined` is a Monty gap, while
`NameError: name 'totl' is not defined` is the model misspelling a variable. Only names in
the `gaps.py` tables count, so ordinary bugs in generated code do not pollute the roadmap.

Every rule was checked against the built worker rather than inferred from `limitations/`,
because the error shapes are not all what the docs imply:

- Unsupported *constructs* surface as `MontyRuntimeError` wrapping `NotImplementedError`, not
  as `MontySyntaxError`. A real `MontySyntaxError` means the model emitted invalid Python.
- A missing module attribute is sometimes reported with the module named
  (`module 'itertools' has no attribute 'groupby'`) and sometimes not
  (`'module' object has no attribute 'sleep'`), so the classifier recovers the module from
  the failing source line in the second case.
- `datetime.timezone.utc` and `datetime.strptime` both work, contrary to what a quick reading
  of `limitations/datetime.md` suggests.
- `%` formatting fails as `TypeError: unsupported operand type(s) for %`, which needs its own
  rule or it reads as ordinary model error.

Gaps carry `certain`, set when the symbol is confirmed against the tables. An uncertain gap
is a guess from the error text and should be triaged by hand before it drives a decision.

In `feature_gaps.md` the decision column is **best prompt still failing**: a gap only the weak
prompts hit is a documentation problem, while a gap `v4_codemode` still hits is a feature to
build.

### What the classifier cannot see

Silent divergences produce no error at all. A generator expression evaluates eagerly to a
`list` in Monty, so `type(x for x in [1]).__name__` is `'list'` and nothing is raised — code
that depends on laziness is simply wrong, and only a wrong answer reveals it. Tasks are the
only instrument for that class of problem, which is another reason they are scored on exact
answers rather than on running without error.

## Prompt variants

| Variant | What it adds |
| --- | --- |
| `v0_bare` | Floor: return one code block, last expression is the result. |
| `v1_current` | The `examples/web_scraper` prompt verbatim, wrong module list included. Baseline. |
| `v2_accurate` | A correct capability list, and a negative list of what is missing. |
| `v3_idioms` | Positive recipes instead: use f-strings, group with `setdefault`, raise builtins. |
| `v4_codemode` | Code-mode strategy: loop in code, `gather`, return only what is needed. |
| `v5_minimal` | The shortest prompt that holds v4's correctness. The cost-axis candidate. |

Two comparisons carry most of the information. **v2 vs v3**: negative enumeration is
expensive and may prime the model toward the very constructs it forbids, so v3 tests whether
positive idiom recipes beat it. **v4 vs v5**: how much of v4 is load-bearing, per axis.

## Automated prompt optimisation (future)

The hand-written variants above are a starting grid, not the finish. The intended follow-up
is to point an automated optimiser — **GEPA**, or DSPy/MIPRO-style search — at this suite,
using the per-axis scores as the fitness signal and letting it evolve the prompt text. That
is out of scope for the current change.

It is also the reason the harness is shaped the way it is, and those choices are worth
keeping:

- Prompts are plain files in `evals/prompts/`, and the runner takes a variant *name*, so new
  prompts can be generated and scored without touching any code.
- Scoring returns a per-axis dict rather than a pass/fail bool, so a search has a gradient to
  follow instead of a single bit.
- `report.py` writes `scoreboard.json` next to the markdown, because an optimiser consumes
  the JSON and a human reads the table.

Two prerequisites are worth naming now. Thirteen tasks is too few for a real train/test
split — an optimiser given all of them will memorise them, so a hold-out set has to come
first. And `--repeat` exists so per-task variance is measurable; without it a search happily
chases noise.

## Where the winning prompt ends up

The suite is a means, not the product. Once a variant wins it should ship in the library, so
callers get a good prompt by default instead of each reconstructing one from `limitations/`:

- `pydantic_monty.SYSTEM_PROMPT` — a static string, for the common case.
- `system_prompt(...)` — a function, added only if the evidence shows the text genuinely must
  vary. Likely axes: `objective` (correctness / cost / time / simplicity, per the table
  above), whether mounts or `os` access are enabled, whether async host functions are in
  play, and appended tool stubs.

**Session persistence is the concrete reason it may have to be a function**, and it is not
hypothetical: `examples/web_scraper` checks out a fresh session per turn so nothing persists
there, while this harness holds one session open so globals survive between turns. Both
prompts are correct about their own integration, and no single static string is correct about
both.

This is deliberately deferred until a variant has actually won — shipping a prompt the suite
has not ranked would defeat the point of having a suite. When it lands it needs a test
asserting the module list in `SYSTEM_PROMPT` matches `StandardLib` in
`crates/monty/src/modules/mod.rs`: silent drift is exactly what happened to the
`examples/web_scraper` prompt, and a shipped prompt drifting is worse than an example doing so.

## Adding a task

Create `evals/tasks/<category>/<name>.py` exporting `TASK = Task(...)`, then make
`--dry-run` pass. Guidance:

- Prefer `Exact`; use `Approx` where floating-point rounding order is a legitimate choice.
  Reach for `Rubric` only when the quality is genuinely subjective — judges add the variance
  this suite exists to remove, and `Predicate` can usually parse the property out instead.
- Host functions are real callables and may be sync or async. Give them latency if the task
  scores round trips.
- A task whose tools keep state between calls must set `setup` to reset it, or the second
  attempt will score differently from the first (see `orchestration/retry_flaky`).
- When a new `limitations/` entry lands and a plausible agent would hit it, it should get a
  task here.

### Type checking shapes what tasks can ask for

Sessions are checked out with `type_check=True`, so the bundled checker runs before the code
does — and it rejects some idiomatic Python. A dict holding a title and two datetimes types
as `dict[str, str | datetime]`, and subtracting two of its values then fails with
`unsupported-operator` before execution. `dates/schedule_conflicts` uses tuples for exactly
this reason; the comment above its reference solution records why.

This is worth watching in scored runs: type-check failures are recorded separately in the
ledger under `type_check`, and a model writing correct-but-unattributable Python is a finding
about the checker, not about the model.

## Known deviations from the plan

- The harness does not use `pydantic_evals`. Its `Case`/`Dataset` model buys little here
  given the custom runner, and the expectation types needed Monty-specific behaviour
  (`needs_judge`, `Every`, gap classification). Everything the plan asked for — evaluators,
  a judge, per-axis reports — is implemented directly, with no new dependency.
- 13 tasks rather than 12; `text/markdown_report` was added because table formatting is where
  `str.format` is hardest to avoid.
