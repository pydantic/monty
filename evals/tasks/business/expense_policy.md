# expense_policy

Decide 40 expense claims against a five-rule policy written in the prompt and three plugin rule functions whose
Python source is in the prompt, to be pasted into the script unchanged.
`rule_vendor` raises `ValueError` when a claim has no notes; that error is recorded per claim and the other rules
still decide it.
Return per-claim decisions with the reasons and errors in a fixed order and wording.

No host functions; `CLAIMS` is an input.
The expected output is computed host-side by the same plugin functions, which `inspect.getsource` also renders into
the prompt.

Scored with `EqualsExpected`.
Monty functions have no `__name__`, so the reference pairs each plugin with its name; `plugin.__name__` raises
`AttributeError`.
`rule_weekend` imports `date` inside the function body.
