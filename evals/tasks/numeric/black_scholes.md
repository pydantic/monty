# black_scholes

Call `fetch_book()` for twelve European options and `fetch_quotes()` for three market prices.
Price every option with Black-Scholes, using `math.erf` for the normal CDF, and back out the implied volatility of each
quoted option by Newton iteration with the analytic vega.
Return `prices` and `implied_vols`, both rounded to four decimal places.

The prompt pins the definitions: no dividends, continuous compounding, Newton from 0.2 until the price difference is
below 1e-8.

Scored with `ApproxExpected` against values computed host-side with the same formulas.
Two host calls are expected, in one batch.
