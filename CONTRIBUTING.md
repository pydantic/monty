# Contributing to Monty

Monty targets CPython 3.14, with known divergences documented in [`limitations/`](limitations/).

Monty is developed with AI assistance, however we also expect contributors to have good oversight of their agents and sufficient understanding to justify any changes proposed.

Due to review capacity we are selective about contributions we accept from outside of the core team.
Please read the following guidelines before submitting a pull request.

Following these guidelines will give your contribution the best possible chance that it is accepted.
We cannot promise that every contribution following these guidelines will be accepted, for example if we are over capacity or if the contribution does not align with our planned roadmap for Monty.

We apologise in advance for any contributions we are not able to accept.
This is not a reflection on your work but rather the reality of the need to prioritise development on Monty appropriately.

The best communication channels to reach the team are GitHub issues and also the `#monty` channel on the [Pydantic Logfire slack](https://logfire.pydantic.dev/docs/join-slack/).

## What we accept

### Small compatibility fixes

Small, focused CPython compatibility fixes are generally welcome, including from first-time contributors.
A good example is a fix demonstrated by an integration test in [`crates/monty/test_cases/`](crates/monty/test_cases/)
that fails on Monty before the fix and passes on both Monty and CPython afterwards.

Keep the PR narrowly scoped, explain the behaviour being corrected, and include a regression test.
Minimise unrelated refactoring.

New stdlib modules or major additions to existing modules are unlikely to be accepted without prior agreement with the maintainers.

### New features

We generally only accept new feature PRs from recognised contributors: people whose previous contributions and
collaboration are known to the maintainers.
If that doesn't describe you, get a maintainer's agreement before starting feature work.
An open feature request is not, by itself, an invitation to submit an implementation.
