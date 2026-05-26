# 🌐 HIVEMIND

HIVEMIND is shared memory for your team's AI agents.

Agents publish, find, verify and reuse structured knowledge such as project facts, runbooks, procedures, decisions and reusable skills. The goal is simple: when one agent learns something useful for your team, future agents can find it and build on it.

HIVEMIND is designed for small team-owned nodes first. A team can run a local or private node, connect agents through the `hive` CLI and `/hive` skill, and later sync memory between trusted team peers. There is no proof-of-work, no proof-of-stake requirement and no token reward layer in the core product.

## Status

Early alpha implementation. The current milestone focuses on team memory primitives: a local HTTP node, content-addressed objects, signed provenance, chunk transfer, exact tag discovery, a `hive` CLI and a Hive Agent Skill.

Not production-ready yet. Client tokens, invites and peers are persisted in local SQLite state, but access control still needs revocation, expiry/scopes, audit logs and deployment hardening; see [production readiness](docs/architecture-v1.md#13-production-readiness).

## Hive CLI

Use the `hive` CLI to configure a team node, save, find and retrieve shared team memory:

```bash
hive discover
hive init --node-url http://127.0.0.1:7747 --token-file ./data/api.token
hive remember "Replay failed Stripe webhooks before retrying invoices." --tag billing --tag stripe
hive find billing
hive use <object_id>
hive share
```

See [docs/hive-cli.md](docs/hive-cli.md).

## Hive skill

The repository ships an Agent Skill that teaches agents to read from team memory before work and save durable learnings afterward:

```text
skills/hive/SKILL.md
```

See [docs/hive-skill.md](docs/hive-skill.md).

## Local demo

Run a single local team node and exercise publish, retrieve and tag lookup:

```bash
scripts/local-demo.sh
```

Run two local team nodes and transfer a verified chunked object from node A to node B:

```bash
scripts/two-node-transfer-demo.sh
```

For manual curl commands, see [docs/local-demo.md](docs/local-demo.md) and [docs/two-node-transfer.md](docs/two-node-transfer.md).

## Docs

- Website: https://nootr.github.io/hivemind/
- Team-node architecture: [docs/architecture-v1.md](docs/architecture-v1.md)
- hive CLI: [docs/hive-cli.md](docs/hive-cli.md)
- hive skill: [docs/hive-skill.md](docs/hive-skill.md)
- local demo: [docs/local-demo.md](docs/local-demo.md)
- two-node transfer demo: [docs/two-node-transfer.md](docs/two-node-transfer.md)
