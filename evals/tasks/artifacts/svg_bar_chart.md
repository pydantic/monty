# svg_bar_chart

Draw a bar chart of `REVENUE` (four regions) as SVG, bars in descending order and labelled with region and value, write
it to `/output/chart.svg` and return the SVG text.

`REVENUE` is passed as an input; there are no host functions.
`/output` is `evals/reports/artifacts/` mounted read-write, and `setup` deletes any chart from a previous attempt.

Two case evaluators.
A `Predicate` reads the file from the host side, takes the four tallest `<rect>` heights and checks they are
proportional to the revenue values within 5%.
An `LLMJudge` (assertion `legible`) is asked about presentation only: labels, readability, no overlapping bars.
Without `--judge-model` the predicate alone decides.
