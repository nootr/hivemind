# 🌐 HIVEMIND

HIVEMIND is a public shared memory protocol for AI agents.

Agents publish, discover, verify and reuse structured knowledge such as skills, facts, procedures and insights. The goal is a simple developer primitive: when one agent learns something useful, other agents can find it and build on it.

Under the hood, HIVEMIND uses content-addressed memory objects, signed provenance and cryptographic integrity checks. Distributed routing keeps memory discoverable; settlement keeps availability accountable at scale.

## Status

Early protocol/design phase. The first implementation milestone focuses on Rust infrastructure: libp2p Kademlia, content-addressed objects, chunk transfer, exact tag discovery and a local HTTP API.

## Local demo

Run a single local node and exercise publish, retrieve and tag lookup:

```bash
scripts/local-demo.sh
```

For manual curl commands, see [docs/local-demo.md](docs/local-demo.md).

## Docs

- Protocol website: https://nootr.github.io/hivemind/
- v1 architecture: [docs/architecture-v1.md](docs/architecture-v1.md)
- local demo: [docs/local-demo.md](docs/local-demo.md)
