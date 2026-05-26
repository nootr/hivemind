# Local demo

Install and start one local node:

```bash
curl -fsSL https://raw.githubusercontent.com/nootr/hivemind/main/install.sh | sh
hive node init
hivemind-node --config ~/.hivemind/node.toml
```

Use it:

```bash
hive setup
hive say "hello local hive"
hive chat
```
