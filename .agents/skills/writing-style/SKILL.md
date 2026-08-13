---
name: writing-style
description: How to write prose that reads like human technical documentation rather than LLM output. Use whenever writing or editing docstrings, comments, limitations/ docs, READMEs, commit messages or PR descriptions, and when prose reads as smooth, salesy or generic.
---

# Writing style

Applies to prose written in this repo: docstrings, comments, `limitations/`,
READMEs, commit messages, PR descriptions, and warnings like the one on
`MountDir`.

The reader is an engineer looking for a fact. Give them the fact. You are not
persuading or building to a conclusion: say what happens, when, and what it
costs.

## Tells to avoid

### Significance instead of mechanism

The most common LLM tell. If a sentence would fit unchanged in any other
project's docs, it carries no information.

- ✗ "This ensures the sandbox remains secure."
- ✓ "Every operation runs relative to a `Dir` opened at mount time, so `..` and
  symlinks cannot reach outside it."

- ✗ "Resource limits provide robust protection against runaway code."
- ✓ "The VM polls allocator usage every 255 instructions; crossing the
  allocator's hard limit exits the worker with `OOM_EXIT_CODE`."

### Throat-clearing

Openers that delay the sentence: "It's worth noting that", "It's important to
understand", "In essence", "Simply put", "At its core", "Let's take a look at".
Delete them; the sentence underneath is the content.

- ✗ "It's worth noting that overlay writes are discarded when the feed ends."
- ✓ "Overlay writes are discarded when the feed ends."

### The "not just X, but Y" reveal

Building to a payoff is an essay move. Docs do not need one.

- ✗ "`heap.rs` isn't just another module — it's the foundation of the entire
  safety model."
- ✓ "`heap.rs` contains the `unsafe` code that `HeapReader` soundness depends
  on. Changes need explicit review."

### Adjectives doing the work of facts

"Powerful", "seamless", "robust", "elegant", "blazing fast", "significantly",
"dramatically". Replace with a number, a mechanism, or nothing.

- ✗ "Overlays are capped at a reasonable size."
- ✓ "`memory_usage_limit` caps retained overlay data at 100 MB by default;
  exceeding it raises `MemoryError` in the sandbox."

### Restating what the reader can see

A docstring that repeats the signature wastes the line it occupies. Say why it
exists, what it costs, or where it bites.

- ✗ "Adds a mount to the mount table."  (on `MountTable::mount`)
- ✓ "Opens the host directory once; later operations run against that
  descriptor, so renaming the path afterwards does not detach the mount."

### Summarising yourself

Do not close a section by restating it, and do not announce what the next
section will do.

- ✗ "In summary, mounts are confined structurally rather than by checking."
- ✓ (nothing, you already said it)

### War stories

Provenance is worth a clause only when it changes what the reader does. How the
bug was found usually does not.

- ✗ "This was demonstrated against a live deployment during Hack Monty, where
  sandboxed code wrote `dataclasses.py` and the client executed it during
  ordinary result conversion."
- ✓ "Sandboxed code can write `json.py` into a read-write mount, or any module
  not yet imported, and have the host's next `import` run it, including imports
  `pydantic_monty` makes itself."

## Smoothness

A different failure from the tells above. Those pad out empty content; this
dresses up real content, which makes it harder to spot and easier to approve.

Sentences engineered for rhythm read as conclusions, so the prose sounds like
it is arguing when it is only listing facts. Balanced clauses and a stressed
last syllable make a sentence sound authoritative whatever it contains, so one
fact wearing three clauses gets read as three facts. Reference prose usually
ends flatly, on a qualifier or a noun phrase, because the writer stopped when
the information ran out rather than when the cadence resolved.

**Timing for suspense.** Commas and subordinate clauses arranged to delay the
point.

- ✗ "The sandbox cannot execute what it writes, but your machine will, later,
  with your privileges, and the path from one to the other is easy to miss."
- ✓ "Files written by sandboxed code stay on the host, where other programs may
  execute them."

**Telling the reader how to feel.** "you did not choose", "easy to miss",
"without being asked", "often does". These supply a mood in place of a fact.

- ✗ "`sys.path[0]` is a directory you did not choose."
- ✓ "`sys.path[0]` is the script's directory, or the cwd for `python -m`,
  `python -c` and the REPL."

**Triples and reversals.** A three-item list where one item carries the fact,
then a `but` clause positioned as the payoff.

- ✗ "Sandboxed code reads, writes and deletes normally and sees its own
  changes, but nothing reaches your disk."
- ✓ "Writes are kept in memory and discarded when the feed ends. Sandboxed code
  still sees its own writes."

**The quotable closer.** A generalisation at the end of a section, memorable,
carrying no new fact. Delete it. The section ends at its last fact.

- ✗ "Principles alone produce prose that follows the rules and still reads like
  an LLM."
- ✗ "Most drafts get better by deleting the first sentence and the last."

**Symmetry for its own sake.** Three-item lists where two items are real,
paragraphs of matched length, every bullet opening with a bolded term. If the
shape came first and the content was fitted to it, cut back to what is true.

Three checks:

- Does the sentence end on a beat? If the last three words could be duller
  without losing meaning, they were there for rhythm.
- Strip the rhythm and count the facts. One is the usual answer.
- Is the sentence about the system, or about how the reader should feel?

## Industry metaphor

Software described as objects moving through space, or as people with
intentions. It is the register of a startup design review, not of reference
documentation; CPython's docs use plain verbs throughout ("raises", "returns",
"is stored in", "propagates", "Changed in version 3.11").

The metaphor also deletes the mechanism. "The error surfaces" does not say
whether it raises, returns or logs. "Wire the tracker through" does not say
parameter, field or global. "It lands in 3.14" does not say merged or released.

Motion and logistics:

| Instead of             | Write                         |
| ---------------------- | ----------------------------- |
| lands, landing         | merged, released in 3.14      |
| ship, shipping         | release                       |
| spin up, stand up      | start, launch                 |
| wire up, plumb through | pass, connect                 |
| thread X through       | pass X as a parameter         |
| bubble up              | propagate, or name the caller |
| surface (verb)         | raise, return, report, log    |
| hand back, hand off    | return, transfer              |
| bake in, baked into    | built in, compiled in         |
| punt on                | defer, skip, leave to         |

Structure as furniture:

| Instead of            | Write                        |
| --------------------- | ---------------------------- |
| seam                  | interface, boundary          |
| surface, surface area | API, the public functions    |
| escape hatch          | override, opt-out            |
| knobs, dials          | options, settings            |
| load-bearing          | required, relied on by X     |
| X-shaped              | with the same interface as X |
| lives in              | is defined in, is stored in  |
| sits on top of        | wraps                        |
| under the hood        | internally                   |

Code with intentions:

| Instead of               | Write                          |
| ------------------------ | ------------------------------ |
| the checker is happy     | the check passes               |
| knows about, is aware of | reads, checks, has a field for |
| talks to                 | sends requests to              |
| teach the parser to      | add X to the parser            |
| wants, expects (of code) | requires                       |
| reaches into             | accesses, reads                |

Also: "for free", "just works", "out of the box", "first-class", "table
stakes", "opinionated", "non-trivial", "unlock", "blast radius", "paper over".

In prose:

- ✗ "Errors from the worker surface to the caller."
- ✓ "`Checkout::feed` returns `PoolError::Crashed` when the worker exits
  without a `FatalError` event."

- ✗ "The tracker is threaded through the whole VM."
- ✓ "Every allocation path takes `&ResourceTracker` as a parameter."

- ✗ "`WorkerTransport` is the `NativeSession`-shaped seam."
- ✓ "`WorkerTransport` has the same methods as `NativeSession`, so
  `session.ts` drives either one."

Terms of art stay, even though they began as metaphors: heap, stack, pointer,
hot path, propagate, boilerplate, tombstone, sandbox escape, attack surface.
`CLAUDE.md` also sanctions foot-gun, happy path and single source of truth. The
test is whether a plain verb would say more than the metaphor does.

## Structure

- Lead with the fact, not the context. First line of a docstring says what the
  thing is.
- Prose for reasoning, bullets for actual lists. A bulleted paragraph is
  harder to read, not easier.
- One idea per sentence. Split anything over about 30 words.
- Comments and field docs: 1 line, 3 at most. Function and struct docstrings:
  5 lines or fewer. If the docstring is longer than the code, something is
  wrong with one of them.
- Warnings: the command or condition first, then the consequence and the
  subtle path to it, then the safe alternative. No preamble. Put the warning
  before the thing it warns about, not after.

## Words and punctuation

| Instead of                       | Write                      |
| -------------------------------- | -------------------------- |
| utilize, leverage                | use                        |
| in order to                      | to                         |
| serves as, acts as, functions as | is                         |
| prior to, subsequent to          | before, after              |
| a variety of, a number of        | several, or the number     |
| allows you to, enables you to    | you can, or the imperative |
| is responsible for handling      | handles                    |

**One term per concept.** Choose the word and keep it. Alternating between
`worker`, `child` and `subprocess` for one thing makes the reader stop to check
whether they are the same thing. Consistency beats variety.

- ✗ "The checkout feeds the child, and the worker replies with events."
- ✓ "The checkout feeds the worker, and the worker replies with events."

**Noun clusters: three words at most.** Longer stacks make the reader guess
which noun modifies which.

- ✗ "parent-side mount table memory usage limit"
- ✓ "the memory limit for a parent-side mount table"

**Active voice and simple tenses.** Use the passive only when the actor is
unknown or irrelevant. Do not drop articles or verbs to save space, except in
the Rust convention of a subjectless first line ("Returns the host path.").

- ✗ "A `PermissionError` will have been raised by the mount."
- ✓ "The mount raises `PermissionError`."

Em dashes should be avoided, or used very sparingly.
But do not update code just to remove em dashes.

Contractions are fine. Second person for user-facing docs ("you can never see
the host path"), imperative for instructions ("mount a dedicated directory").
Hedges ("generally", "typically", "may") are for genuine uncertainty; if the
behaviour is defined, state it.

## Checking a draft

- Could this sentence appear verbatim in another project's docs? Then it is
  empty.
- Can you point at the code each claim describes? If not, you are guessing.
- Delete the first sentence. Was anything lost?
- Read it aloud. Sentences that sound like a conference talk get cut.
- Strip a sentence's rhythm. How many facts are left?
- Would a reviewer learn anything they could not get from the signature?
