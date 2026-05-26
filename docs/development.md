# Development

## Git hooks

This repository includes a pre-commit hook in `.githooks/pre-commit`.

Enable it once per clone:

```bash
git config core.hooksPath .githooks
chmod +x .githooks/pre-commit
```

After that, every `git commit` runs:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
for test_script in ./e2e-tests/*.sh; do "$test_script"; done
```

GitHub Actions runs the same hook in CI, so the E2E shell tests are part of pull-request validation.

To run the same checks manually:

```bash
.githooks/pre-commit
```

If the hook fails, fix the reported issue and commit again. Do not bypass the hook unless you have a specific reason and mention it in the commit or review notes.
