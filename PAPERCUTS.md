2026-08-05T00:15:57Z - Claude Code - claude-opus-5

Ran 'npm ci' in crates/monty-js to typecheck docs TS snippets; it fails with EUSAGE because package-lock.json pins the @pydantic/monty-<platform> optional deps at an empty version while package.json wants 0.0.19. 'npm install' would fix it but rewrites the tracked lockfile, so there is no way to get JS devDeps (tsc, prettier) from a clean checkout without dirtying the tree.

