# followup_reuse

Turn one: call `fetch_orders()` for seven orders and return total revenue rounded to two decimal places.
Turn two, in the same session: return the region with the highest revenue as a `region`/`amount` dict.

Both turns are scored with `Approx`.
Turn one expects one host call; the follow-up expects zero, because the orders are still bound in the session.
A follow-up that calls `fetch_orders()` again gets the right answer and still fails.
