# redact_pii

Parse the sender name and address out of `Name <email>` headers on 15 emails, redact phone numbers, the address and
the name in each body, and list the distinct senders after NFKC normalisation and casefolding.
Names are in Latin with accents, Cyrillic, Greek, Vietnamese and CJK, and repeat in fullwidth, upper-case and
decomposed forms.

No host functions; `EMAILS` is an input.
Scored with `EqualsExpected` against the same regexes and normalisation run under CPython.

The reference currently fails: CPython's `casefold()` maps the Greek final sigma `ς` to `σ`, Monty's does not, so
the sender `πέτρος παπαδόπουλος` differs in one character.
The case is kept as designed so the divergence is measured.
