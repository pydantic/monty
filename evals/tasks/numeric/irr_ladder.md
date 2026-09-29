# irr_ladder

Call `fetch_loans()` for four loans (`principal`, `annual_rate`, `term_months`).
Compute each loan's level monthly payment, its NPV at `DISCOUNT_RATE` and its IRR by bisection, then build the full
amortisation schedule for `SCHEDULE_LOAN` with interest, principal and balance rounded to cents every month.
Return the NPVs, IRRs, the payment, the first, second and last schedule rows, and total interest.

The prompt pins every formula and rounding step, so the 360-row schedule must reproduce the host's cents exactly.
`decimal` would be the natural tool for that; Monty does not bundle it, so the reference rounds with `round`.

Scored with `ApproxExpected`; one host call is expected.
