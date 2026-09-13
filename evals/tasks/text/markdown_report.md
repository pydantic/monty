# markdown_report

Call `fetch_regional_summary()` for four rows of `region`, `revenue` and `orders` and render them as a markdown table,
highest revenue first.

The prompt specifies the layout exactly: revenue with thousands separators and two decimals, `Region` left-aligned, the
numeric columns right-aligned, every cell padded to the widest cell in its column including the header, and a `| :--- | ---: | ---: |` separator whose dash runs match the column widths.

Scored with `EqualsExpected` against the rendered string; one host call is expected.
