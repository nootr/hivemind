# Hive CLI

`hive` is the command-line entrypoint for HIVEMIND team memory.

The CLI talks to a team-owned `hivemind-node` HTTP API. The node may run on your machine, on a private team server, or behind an internal gateway. The CLI does not start or manage the node yet.

Status: alpha/local team prototype. Client tokens, invites, peers and audit events are persisted in node SQLite state. Client tokens have expiry, revocation and a memory scope, but production access control still needs narrower scope policy and deployment hardening.

## Configuration

Configure the CLI once with a node URL and token file:

```bash
hive init --node-url http://127.0.0.1:7747 --token-file ./data/api.token
```

This writes `~/.config/hivemind/hive.json` with owner-only permissions on Unix systems. Override the path with `--config` or `HIVEMIND_CONFIG`.

Environment variables are still supported:

```bash
export HIVEMIND_NODE_URL="http://127.0.0.1:7747"
export HIVEMIND_API_TOKEN="$(tr -d '\n' < ./data/api.token)"
```

When no team node is configured, the CLI prints join/init/share guidance instead of only failing on auth.

## Commands

Remember a text memory for the team:

```bash
hive remember "Replay failed Stripe webhooks before retrying invoices." \
  --tag billing \
  --tag stripe \
  --tag runbook
```

Find team memories by exact tag:

```bash
hive find billing
```

Use a memory by object ID:

```bash
hive use <object_id>
```

## Discover, join and share

Find Hive nodes that announce themselves on the local network:

```bash
hive discover
```

Discovery is an airdrop-style convenience for changing local IPs. It is not trust and does not grant access; ask a teammate/admin for an invite before joining a discovered node.

Show share guidance for the configured node:

```bash
hive share
```

If the node URL is loopback-only, `hive share` explains that it cannot be shared directly and suggests a private reachable URL:

```text
This node is configured as local-only:
  http://127.0.0.1:7747

To share it with teammates, expose the node on a private reachable URL, then run:
  hive share --node-url https://hive.your-team.internal
```

For a reachable private node, `hive share` asks the node for a short-lived invite and prints a copy-paste command:

```text
This node is available at:
  https://hive.your-team.internal

Share with a teammate:
  hive join 'hive://join?node=https%3A%2F%2Fhive.your-team.internal&invite=ABCD-EFGH-IJKL'
```

A teammate joins with the link or with a code if their node URL is already configured:

```bash
hive join 'hive://join?node=https%3A%2F%2Fhive.your-team.internal&invite=ABCD-EFGH-IJKL'
hive join ABCD-EFGH-IJKL
```

Security rule: shared URLs must not contain the admin API token. Invite links contain a short-lived, limited-use invite code that `hive join` exchanges for a generated client token in local config. Generated client tokens expire and can be revoked by an admin through the node API. Admin security events are available from `GET /v1/audit`.

## Peer candidates and trust

When a node accepts `hive join`, it can share known peer node URLs and node IDs/public-key fingerprints. The CLI stores those as **untrusted peer candidates**.

```bash
hive peers
hive peer trust <node-id>
hive peer untrust <node-id>
```

Trust is based on node ID/public-key fingerprint, not URL or IP address. Agents must not trust peer candidates automatically. If a task requires trusting a peer, the agent should ask the user first and only run `hive peer trust ...` after explicit approval.

## Notes

- `remember` currently publishes a `fact` object with `text/plain` payload.
- `find` currently uses exact tag lookup.
- `use` expects a UTF-8 text payload.
- The CLI is for team/workspace memory. Do not store secrets, credentials or private keys.
