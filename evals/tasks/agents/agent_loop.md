# agent_loop

A ReAct-style support agent written as a loop in the sandbox.
The code keeps a `messages` list, calls `call_llm(messages)`, parses the one-line JSON reply
(`{"tool": name, "args": {...}}` or `{"final": {...}}`), runs the named host tool, appends the result as a `tool`
message and repeats, for at most eight turns.
The request is a refund on order O-1001 for customer C-42.

Host tools: `lookup_order`, `lookup_customer`, `compute_refund`, plus `call_llm` from the harness.
Under `--dry-run` the stub plays the model: it counts the `tool:` lines in the rendered transcript and returns the
next scripted step, ending with the final object.

Scored with `ApproxExpected` on the final dict, `result_size` (200 bytes), and a pin of seven host calls in seven
round trips, since every step depends on the previous reply.
