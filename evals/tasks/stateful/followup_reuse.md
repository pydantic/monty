# followup_reuse

Turn one: call `fetch_orders()` for seven orders and return total revenue rounded to two decimal places.
Turn two, in the same session: return the region with the highest revenue as a `region`/`amount` dict.

Turn one is scored with `ApproxExpected`; the `FollowUp` evaluator scores turn two's result from the case attributes
with the same tolerance.
Turn one expects one host call; the follow-up expects zero, because the orders are still bound in the session.
A follow-up that calls `fetch_orders()` again gets the right answer and still fails `follow_up_calls_as_expected`.
