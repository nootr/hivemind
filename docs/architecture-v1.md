# HIVEMIND Team Node Architecture v1

HIVEMIND is shared memory for a team's AI agents. It gives agents a common place to publish, find, verify and reuse team knowledge such as facts, runbooks, procedures, decisions, insights and reusable skills.

This architecture replaces the earlier public-network / incentive-network framing. The v1 product is team-scoped: small nodes owned by a team, with optional peer-to-peer sync between trusted team nodes. There is no proof-of-work, proof-of-stake, token reward or settlement layer in the core design.

## 1. Product scope

Primary user-facing flow:

```text
agent + /hive skill
    -> hive CLI
    -> local/team HIVEMIND node
    -> team memory store
```

The first implementation proves these primitives:

1. publish a memory object
2. retrieve by object ID
3. find by tag
4. export/import verified envelopes and chunks between nodes
5. use the CLI and skill as the product entrypoint

Later team-node sync automates the transfer flow between trusted peers.

## 2. User profiles

### Agent users

They want their agents to stop forgetting project facts, runbooks and decisions. They interact with `/hive`, not with storage internals.

### Team admins

They run or configure the team node. Their goal is reliable private team memory, not mining, staking or public rewards.

### Developers and integrators

They use the HTTP API and CLI to integrate team memory into coding agents, MCP servers, workflow engines and agent frameworks.

## 3. Node model

A HIVEMIND node is a lightweight local or private service.

Responsibilities:

- accept authenticated local/team API requests
- store canonical object and chunk bytes
- index metadata, tags and references
- verify signed objects and chunk content
- export and import transfer envelopes
- later: sync selected memory with trusted team peers

A node does not need to participate in a global network to be useful.

## 4. Trust model

V1 assumes a team-controlled trust boundary:

- `/health` is unauthenticated
- `/v1/*` requires bearer auth
- API tokens and agent keys are local secrets
- imported objects are verified before storing
- chunk bytes must match their content IDs
- object authorship is signed

The node should be deployed behind local access controls, a VPN, private network, or a team gateway. Public unauthenticated exposure is not a goal.

## 5. Architecture style

The implementation uses Rust with a hexagonal workspace layout:

```text
crates/
  hivemind-core        # domain types, canonical encoding, IDs, signatures
  hivemind-app         # use cases and port traits
  hivemind-adapters    # filesystem and SQLite adapters
  hivemind-node        # HTTP node composition/runtime
  hivemind-cli         # hive CLI product entrypoint
skills/
  hive/                # Agent Skill using the hive CLI
```

Dependency rules:

```text
core -> no app/adapters/node imports
app -> core
adapters -> app + core
node -> app + adapters + core
cli -> HTTP API DTOs/client behavior
skills -> CLI instructions only
```

Core remains synchronous and runtime-independent. Node and adapters use Tokio where needed.

## 6. Object model

Object kinds:

```text
skill | fact | procedure | insight | rating | report | tombstone | alias
```

Knowledge-specific validation is intentionally light in v1. The shared memory layer stores portable, verifiable objects; agent workflows decide how to use them.

Object IDs are computed over the unsigned canonical body with domain separation:

```text
object_id = blake3("hm-object-v1" || deterministic_cbor(ObjectBody))
```

The signed envelope wraps the body:

```text
ObjectEnvelope {
  object_id,
  body,
  author_public_key,
  author_signature
}
```

Object signatures sign the object domain, object ID and canonical body:

```text
author_signature = sign("hm-object-signature-v1" || object_id || deterministic_cbor(ObjectBody))
```

Payload model:

```text
Payload::Inline {
  mime_type,
  bytes
}

Payload::Chunked {
  mime_type,
  total_size,
  chunks: Vec<ChunkRef>
}
```

Inline objects are used up to `16 KiB`. Larger payloads are chunked with default `64 KiB` chunks.

Chunk IDs:

```text
chunk_id = blake3("hm-chunk-v1" || chunk_bytes)
```

## 7. Storage model

Canonical content is stored on the filesystem:

```text
objects/<prefix>/<object_id>.cbor
chunks/<prefix>/<chunk_id>
```

SQLite stores local metadata and indexes:

- objects
- chunks
- object_chunks
- tags
- object_references

Filesystem content is the canonical truth. SQLite is an index that can be rebuilt from content when needed.

## 8. HTTP API

Authenticated routes:

```text
POST /v1/objects
GET  /v1/objects/{object_id}
GET  /v1/objects/{object_id}/envelope
GET  /v1/objects/{object_id}/referrers
POST /v1/objects/envelope
POST /v1/objects/envelope/plan
GET  /v1/chunks/{chunk_id}
PUT  /v1/chunks/{chunk_id}
POST /v1/invites
GET  /v1/peers
POST /v1/peers
GET  /v1/tags/{tag}
```

Unauthenticated route:

```text
GET  /health
POST /v1/join
```

The API is a team/local control plane. It is not a public trust boundary.

## 9. CLI and skill UX

The `hive` CLI is the product entrypoint for agents and scripts:

```bash
hive init --node-url http://127.0.0.1:7747 --token-file ./data/api.token
hive remember "..." --tag project --tag runbook
hive find project
hive use <object_id>
hive share
```

The Hive Agent Skill teaches agents to:

1. check team memory when existing context may help
2. retrieve relevant memories before acting
3. save durable learnings after a task
4. avoid saving secrets, transient status or guesses
5. continue gracefully if memory is unavailable

### Join/share setup UX

The CLI makes team-node bootstrapping explicit:

```bash
hive discover
hive init --node-url http://127.0.0.1:7747 --token-file ./data/api.token
hive share
hive join <invite-link-or-code>
```

If a user runs `hive remember`, `hive find` or `hive use` without configuration, the CLI explains how to join or initialize a team node:

```text
No Hive team node configured.

Join a team node:
  hive join <invite-link-or-code>

Or configure manually:
  hive init --node-url http://127.0.0.1:7747 --token-file ./data/api.token

Running your own node?
  hive share
```

`hive discover` uses UDP broadcast as an airdrop-style convenience for local IP changes. Discovery only returns candidate node URLs; it does not grant access and does not imply trust.

`hive init` writes local CLI config. `hive share` shows whether the configured node URL is local-only or shareable. For reachable nodes it asks `POST /v1/invites` for a short-lived, limited-use invite and prints a `hive join ...` command.

Invite links must not include the admin API token. They carry a short-lived invite code that `hive join` exchanges through `POST /v1/join` for a generated client token in local config.

The join response can include known peer node URLs and node IDs/public-key fingerprints. The CLI stores these as untrusted peer candidates. Trust is based on node ID, not URL or IP address. Trust is local and manual: agents must ask the user before running `hive peer trust <node-id>`.

## 10. Team peer sync roadmap

The current two-node demo manually performs the future sync protocol:

1. source exports a signed envelope
2. target plans import and reports missing chunks
3. target retrieves chunks from source
4. target imports the envelope
5. target indexes the object locally

Future peer sync should automate this between trusted team nodes.

Recommended v1 peer-sync shape:

```text
[team]
team_id = "engineering"

[peers]
allow = ["node-a", "node-b"]
bootstrap = ["https://node-a.internal:7747"]
```

Sync should be team-scoped and allowlisted. Discovery can start with static peers or team-local service discovery. A global DHT is not required for the product value.

## 11. Privacy and data boundaries

HIVEMIND stores what agents publish. Teams should treat it like shared internal documentation plus agent memory.

Rules:

- do not store secrets or credentials
- sanitize sensitive operational details when possible
- prefer concise durable knowledge over raw logs
- use private deployment boundaries for private memory
- expose provenance so agents can judge source and age

## 12. Non-goals for v1

- global public memory network
- proof-of-work
- proof-of-stake
- token rewards
- settlement or slashing
- permissionless storage-provider marketplace
- semantic truth arbitration

These can be explored separately later if the team-memory product proves useful, but they are not part of the production path for v1.

## 13. Production readiness

This implementation is an alpha/local team prototype, not production-ready.

Production blockers:

- Persist client tokens, invite state, peer registry and trust decisions beyond process lifetime.
- Add client-token expiry, revocation and narrower scopes.
- Add audit logs for invite creation, join exchanges and trust changes.
- Add a clear node public-key/fingerprint confirmation UX before trust.
- Harden UDP discovery with rate limits, validation and deployment guidance for VPNs/subnets.
- Implement trusted peer sync; current two-node flow is still manual transfer.
- Add packaging/install flows for node and CLI.
- Add backup/restore docs and config/state migration/versioning.
- Add observability and private deployment guidance, including TLS/proxy recommendations.

## 14. Near-term production path

1. Persist node peer registry, invite state and client tokens beyond process lifetime.
2. Add client-token revocation, expiry and narrower scopes.
3. Add trusted team peer sync.
4. Package the node and CLI for local/team installation.
5. Add better search beyond exact tags.
6. Add update/supersede/tombstone UX for memory hygiene.
7. Add team/workspace configuration.
8. Add admin docs for private deployment, backups and migrations.
