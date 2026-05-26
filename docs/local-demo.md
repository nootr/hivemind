# Local demo

Start one local node:

```bash
cargo run -p hivemind-node -- --data-dir ./data --bind-addr 127.0.0.1:7747 --public-url http://127.0.0.1:7747
```

Use it:

```bash
cargo run -p hivemind-cli -- setup
cargo run -p hivemind-cli -- say "hello local hive"
cargo run -p hivemind-cli -- chat
```
