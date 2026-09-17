# ledger_match

Call `fetch_bank_lines()` for 60 statement lines and `fetch_invoices()` for 55 invoices.
Match each bank line to an unmatched invoice: first by exact amount within three days, then by amount within 1% when the
invoice number's digits appear in the bank reference.
Return the matches in bank order, the unmatched bank lines and the unmatched invoices.

The fixture mixes exact payments, payments a few days late, short payments within 1%, references with the invoice number
reformatted, ten bank fees that match nothing, and five unpaid invoices.
The prompt pins the pass order and tie-breaks, so the answer is unique.

Scored with `EqualsExpected`; two host calls are expected, in one batch.
