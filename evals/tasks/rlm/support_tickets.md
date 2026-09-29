# support_tickets

The Recursive Language Models case.
`context` is a generated support-ticket log, about 4,000 tickets and 600 KB, bound as an input: a header line
`--- <id> | <date> | <customer> | <tier>` then one line of customer text.
The question covers only gold-tier tickets opened in August 2026: the most common issue type, and the customer
who raised the most of that type.
Return `issue_type`, `count`, `top_customer` and `top_customer_count`.

The text never names the issue type and each type has four phrasings, so grepping for label words fails.
The model has to filter the log in Python, then classify the remaining tickets with `llm_query`, the host function
the runner adds to any task with a `sub_model_stub`.
With a model, `llm_query` is one plain completion of that model; under `--dry-run` the stub labels every known
ticket body it finds in the prompt.

Scored with `ApproxExpected`, `result_size` (200 bytes) and `within_call_budget`: the budget is one sub-call per
selected ticket gathered twenty at a time, so a sequential loop fails it.
Run it with `--mode repl` to get the peek-then-act loop the RLM paper describes.
