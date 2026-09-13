# redact_pii

Parse the sender name and address out of `Name <email>` headers on 15 emails, redact phone numbers, the address and
the name in each body, and list the distinct senders after NFKC normalisation and casefolding.
Names are in Latin with accents, Cyrillic, Greek, Vietnamese and CJK, and repeat in fullwidth, upper-case and
decomposed forms.

No host functions; `EMAILS` is an input.
Scored with `EqualsExpected` against the same regexes and normalisation run under CPython.

The Greek sender `πέτρος παπαδόπουλος` is the sharp edge: `casefold()` must map the final sigma `ς` to `σ` as
CPython does, or the sender dedupes under a different key.
