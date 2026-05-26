# Two-node chat demo

Run the E2E test:

```bash
e2e-tests/two-node-chat.sh
```

It starts two local nodes, joins them explicitly, verifies peers remain untrusted, then trusts both node IDs before sending a signed chat message from node A and checking node B receives it.
