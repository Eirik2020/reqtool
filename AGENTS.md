# reqtool - agent entry

Requirements and verification tracing for Rust projects.
Public repo: nothing private, no secrets, no name of a user's project.

## Authority

- This file is the highest authority in this repository; only the owner outranks it. A document that diverges from it is wrong: fix it.
- No agent writes to an `AGENTS.md` or `CLAUDE.md`, here or in any folder, without the owner's review of the exact text in chat and in the PR. Never split one.
- `README.md` states the rules reqtool implements and every command. Code that diverges from it is a bug; changing a rule needs the owner's decision.

## Commands

```
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Before every push, run all three: CI fails the change on each. A local pass is not
"fixed": only green repo CI on the pushed head is. When repo CI fails, read the failing
job's own log first and fix every failure it shows before pushing again.

## Rules

1. reqtool is generic: no project that uses it is named in code, tests or documents.
2. reqtool decides what a change owes in every project that uses it. A change to what it flags as owed states, in the PR, which statuses change and why.
3. A change to status, hashing or tracing comes with a test in `tests/`.
4. A behaviour change updates `README.md` in the same change. Nothing documented only in a commit, PR or chat.
5. One commit per item, detailed body; rustfmt; dates YYYY-MM-DD.
