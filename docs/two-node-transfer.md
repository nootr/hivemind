# Two-node chat demo

Run the E2E test:

```bash
e2e-tests/two-node-chat.sh
```

It starts two local nodes, joins them explicitly, verifies peers remain untrusted, sends a signed chat message from node A and checks node B receives it.
