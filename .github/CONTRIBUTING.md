# Contributing

`main` only changes through pull requests (DEC-0019). Branch protection enforces this for every account, administrators included. Enable the local guard once per clone:

```bash
git config core.hooksPath .githooks   # pre-push refuses a push to main
```

Create a focused branch named `feat/…`, `fix/…`, `docs/…` or `chore/…` for every change, and open a pull request to `main`. Each PR should:
- reference specification sections and decision IDs;
- describe failure and rollback behaviour;
- add tests;
- update `docs/CHANGELOG.md`.

A PR merges when the required `documentation`, `rust` and `contracts` checks pass on an up-to-date branch. No approving review is required while the project has one maintainer.

Before opening a PR, run what CI runs:

```bash
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --workspace
RUSTFLAGS="-D warnings" cargo llvm-cov --locked --workspace --summary-only --fail-under-lines 26
                                         # runs the tests; set DATABASE_URL for the database suites
bash scripts/check-docs.sh
```

Warnings are errors everywhere. Never bypass a required check, and never merge unresolved security, risk, accounting or compatibility concerns.

Commit messages follow Conventional Commits with an imperative subject, and explain why when the change is not obvious. Generated files identify their generator and source revision. Keep pull requests small enough to review against one traceability objective.
