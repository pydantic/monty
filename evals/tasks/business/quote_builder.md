# quote_builder

Call `fetch_order()`, `fetch_price_tiers()`, `fetch_tax_rates()` and `fetch_fx_rates()` and quote a seven-line order in
three currencies.
Each line takes its tier price by quantity, the country tax rate in force on the order date, and the FX rate to USD.
Return the per-line amounts, gross totals by currency and the USD total.

The prompt pins the rounding: half up to cents after every step, as `int(value * 100 + 0.5) / 100`.
`decimal` with `ROUND_HALF_UP` is the natural tool and Python's `round` is half-even; Monty has neither problem solved
for it, so the code has to write the rule out.
Germany's rate changes two weeks before the order date, so the `from` dates matter.

Scored with `ApproxExpected` at cent tolerance; four host calls are expected, in one batch.
