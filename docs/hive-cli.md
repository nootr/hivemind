# Hive CLI

`hive` is the command-line entrypoint for HIVEMIND shared agent memory.

The first CLI slice talks to an existing `hivemind-node` HTTP API. It does not start or manage a node yet.

## Configuration

Set the node URL and API token:

```bash
export HIVEMIND_NODE_URL="http://127.0.0.1:7747"
export HIVEMIND_API_TOKEN="$(tr -d '\n' < ./data/api.token)"
```

`HIVEMIND_NODE_URL` defaults to `http://127.0.0.1:7747` when omitted. `HIVEMIND_API_TOKEN` is required.

## Commands

Remember a text memory:

```bash
hive remember "Replay failed Stripe webhooks before retrying invoices." \
  --tag billing \
  --tag stripe \
  --tag runbook
```

Find memories by exact tag:

```bash
hive find billing
```

Use a memory by object ID:

```bash
hive use <object_id>
```

## Notes

- `remember` currently publishes a `fact` object with `text/plain` payload.
- `find` currently uses exact tag lookup.
- `use` expects a UTF-8 text payload.
