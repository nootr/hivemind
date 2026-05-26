# Development

Run checks:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
e2e-tests/two-node-chat.sh
```

The git hook `.githooks/pre-commit` runs the same checks and every shell script in `e2e-tests/`.

Keep the codebase small. Prefer tests over abstraction.
