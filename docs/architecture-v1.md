# HIVEMIND Architecture v1

HIVEMIND is a public, decentralized, agent-native shared memory network. Agents and nodes publish, discover, transfer, verify and eventually reward structured knowledge objects such as skills, facts, procedures and insights.

This document captures the v1 design decisions.

## 1. Scope and build order

HIVEMIND v1 starts with infrastructure, not skills:

1. DHT/content routing and transfer
2. PoS-facing interfaces, PoA and PoSI scoring
3. Knowledge-specific skill semantics

The first implementation target is a vertical infrastructure slice proving two-node DHT discovery, transfer and verification for both inline and chunked objects.

## 2. Node model

There is one lightweight Rust binary for all node types. All capabilities are present in the binary, but runtime behavior is explicitly enabled via config.

Capabilities:

- client behavior: publish, fetch, tag lookup
- storage provider behavior: store/provide object and chunk content
- auditor behavior: run deterministic audits and submit results
- mock settlement behavior: local development/testing only

Client nodes do not need cryptocurrency incentives. Storage providers and auditors can later bind to stake identities for rewards and settlement.

## 3. Architecture style

The implementation uses Rust with hexagonal architecture and a workspace layout.

Workspace crates:

```text
crates/
  hivemind-core        # domain types, canonical encoding, crypto abstractions
  hivemind-app         # use cases/services + port traits
  hivemind-adapters    # fs/sqlite/mock/libp2p/http adapters
  hivemind-node        # binary composition/config/runtime
  hivemind-proto       # protobuf wire messages
```

Dependency rules:

```text
core -> no app/adapters/node imports
app -> core
adapters -> app + core
node -> all crates
proto -> wire types only
```

Core is synchronous and runtime-independent. Application ports are async. Adapters and node runtime use Tokio.

Primary ports:

- `ContentRoutingPort`
- `ChunkStorePort`
- `ObjectStorePort`
- `ProtocolParamsPort`
- `StakeRegistryPort`
- `SettlementPort`
- `IdentityPort`
- `ClockPort`
- `EventBusPort`

## 4. Protocol parameters

The DHT reads operational protocol parameters through a chain-state-like interface. In v1 this is backed by a mock/local implementation.

Protocol constants are hardcoded per protocol version:

- protocol version
- canonical serialization rules
- hash algorithm
- object/chunk ID derivation
- signature scheme
- maximum chunk size

Operational params come from `ProtocolParamsPort`:

- default chunk size: `64 KiB`
- inline object threshold: `16 KiB`
- max payload size: `10 MiB`
- replication/provider settings
- TTLs
- audit intervals
- scoring weights

## 5. Cryptography and encoding

Hashing:

```text
BLAKE3 with explicit protocol/domain separation
```

Domain-separated identifiers:

```text
object_id = blake3("hm-object-v1" || deterministic_cbor(ObjectBody))
chunk_id  = blake3("hm-chunk-v1" || chunk_bytes)
agent_id  = blake3("hm-agent-v1" || agent_public_key)
alias_key = blake3("hm-alias-v1" || owner_agent_id || alias_slug)
tag_key   = blake3("hm-tag-v1" || normalized_tag || bucket)
```

Signatures:

```text
network identity: Ed25519/libp2p
agent author identity: Ed25519 in v1
stake signatures: chain-native abstraction later
```

Signed payloads also use domain separation. Object signatures sign the object domain, object ID and canonical body:

```text
author_signature = sign("hm-object-signature-v1" || object_id || deterministic_cbor(ObjectBody))
```

Canonical object encoding:

```text
deterministic CBOR via minicbor
```

Wire/network request-response encoding:

```text
protobuf messages carrying canonical CBOR object bytes where needed
```

## 6. Identity model

Identities are separated:

```text
AgentId / AuthorId = blake3("hm-agent-v1" || agent_public_key)
ProviderId = libp2p PeerId
StakeId = optional chain account identity
```

Authorship is independent from network transport identity. Publishing does not require stake. Storage/reward behavior can require stake later.

A storage provider can bind network and stake identities by proving ownership of both keys.

V1 key files:

```toml
[identity]
agent_key_path = "./data/agent.ed25519"
network_key_path = "./data/network.ed25519"
```

Keys are generated if missing and persisted to keep stable `AgentId` and `PeerId`.

## 7. Object model

Object kinds supported by infra-v1:

```text
skill | fact | procedure | insight | rating | report | tombstone | alias
```

Knowledge-specific validation is not part of infra-v1.

Object IDs are computed over the unsigned canonical body with domain separation:

```text
object_id = blake3("hm-object-v1" || deterministic_cbor(ObjectBody))
```

The envelope signs the object body externally:

```text
ObjectEnvelope {
  object_id,
  body,
  author_signature
}
```

The author is part of `ObjectBody`, therefore authorship/provenance is part of object identity.

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

Chunk IDs:

```text
chunk_id = blake3("hm-chunk-v1" || chunk_bytes)
```

Inline objects are used up to `16 KiB`. Larger payloads are chunked with default `64 KiB` chunks.

Timestamps:

- `ObjectBody.created_at_ms` is author-declared and signed
- local stores also keep `received_at_ms`
- future timestamps beyond 1 hour clock skew are rejected

## 8. Storage

Canonical content is stored on the filesystem. Metadata and indexes are stored in SQLite.

Filesystem content store:

```text
data/
  chunks/<first2hex>/<chunk_id_hex>
  objects/<first2hex>/<object_id_hex>.cbor
```

SQLite stores indexes/metadata:

- objects
- chunks
- object_chunks
- provider records/cache
- tag index cache
- attestation index cache
- audit results
- provider scores

SQLite is not the source of content truth. Content truth is always verified cryptographically:

```text
object_id = blake3("hm-object-v1" || canonical body)
chunk_id = blake3("hm-chunk-v1" || chunk bytes)
```

Filesystem adapter uses Tokio filesystem APIs and atomic writes:

1. write temp file
2. optionally flush/sync
3. rename to final content-addressed path
4. existing final path is idempotent success

## 9. Networking

Base stack:

```text
libp2p Kademlia
```

Required protocols/features:

- TCP
- Noise
- Yamux
- Identify
- Ping
- Kademlia
- Request-response
- mDNS for local development

Bootstrap:

- static bootstrap peers from config
- mDNS for local dev

Example config:

```toml
[network]
listen_addrs = ["/ip4/0.0.0.0/tcp/0"]
bootstrap_peers = []
enable_mdns = true
```

## 10. DHT records and discovery

Kademlia is used for exact content routing, not full-text search.

DHT routing keys:

```text
/hm/v1/object/<object_id>
/hm/v1/chunk/<chunk_id>
/hm/v1/tag/<normalized_tag>/<bucket>
```

Object and chunk lookup use provider records.

Tag lookup uses provider records for tag-bucket servers. The actual tag bucket candidates are fetched via request-response from providers. This avoids unsafe shared append-only Kademlia value records.

A storage provider serves tag buckets for objects it locally hosts.

Tag semantics:

- tags are exact-match discovery labels
- not full-text search
- free tags are allowed with strict syntax
- canonical tag registry/aliases can come later

Tag normalization v1:

```text
lowercase ASCII slug
allowed: [a-z0-9-_.]
length: 1..64
max tags per object: 32
```

Tag buckets:

```text
tag_bucket_bits = 4
bucket_count = 16
max_records_per_bucket_response = 128
tag_bucket_provider_query_count = 3
```

Tag records are candidates until the object is fetched and verified. Final validity requires:

- valid object envelope
- object body contains the tag
- object/chunks are retrievable and valid

## 11. Request-response protocols

Minimum v1 protocols:

```text
/hm/1/object/get
/hm/1/chunks/get
/hm/1/tag-bucket/get
```

`GetChunks` is batch-based. A single chunk fetch is a batch with one item.

Fetch flow:

1. find object providers via DHT
2. fetch object envelope
3. verify object ID and author signature
4. for chunked payloads, find providers per chunk ID via DHT
5. fetch chunks via batch request-response
6. verify every chunk hash
7. assemble payload

## 12. HTTP API

The node exposes a local HTTP JSON API for agents.

Framework:

```text
axum
```

API binds to localhost by default and uses bearer auth for `/v1/*` endpoints.

```toml
[api]
bind_addr = "127.0.0.1:7747"
auth_token_file = "./data/api.token"
```

Auth:

```http
Authorization: Bearer <token>
```

`GET /health` is unauthenticated liveness only.

Initial endpoints:

```text
GET  /health
POST /v1/objects
GET  /v1/objects/{object_id}
GET  /v1/tags/{tag}
```

`POST /v1/objects` uses JSON with base64 payload:

```json
{
  "object_type": "fact",
  "mime_type": "text/plain",
  "payload_base64": "...",
  "tags": ["rust", "libp2p"]
}
```

`GET /v1/objects/{object_id}` returns JSON with base64 payload and verification metadata.

## 13. Ratings, reports, tombstones and aliases

Attestations are signed, content-addressed first-class objects discoverable through an attestation index.

Attestation index key:

```text
/hm/v1/attestations/<target_object_id>
```

Attestation types:

- rating
- report
- tombstone

Ratings:

```text
score = -1 | 0 | +1
```

There is one active rating per rater per object. Newer valid ratings by the same rater supersede older ones for aggregation, while old immutable records may still exist.

Ratings are for semantic quality/usefulness, not storage correctness.

Reports are for safety/moderation signals:

- spam
- malware
- false
- unsafe
- illegal
- other

Reports do not delete content. They influence local risk status through trust/reputation policy.

Tombstones allow the original author to retract/deprecate their own object.

Aliases:

- immutable objects can use `supersedes` references
- mutable aliases point to latest/curated object IDs
- v1 aliases are agent-owned
- global canonical aliases require a later registry/governance layer
- aliases may point to objects by any author, but provenance must remain explicit

Alias lookup is exact by namespaced alias:

```text
/hm/v1/alias/<owner_agent_id>/<alias_slug>
```

## 14. PoA, PoSI and scoring

PoA means availability and latency.

PoSI means Proof-of-Storage-Integrity: cryptographic storage integrity, not semantic truth.

Semantic quality is measured separately via ratings/attestations.

Availability score:

```text
success_rate = successful_retrieval_audits / total_retrieval_audits
latency_score = clamp(target_latency_ms / p95_latency_ms, 0.0, 1.0)
A = success_rate * latency_score
```

Defaults:

```text
rolling_window = 7 days
target_latency_ms = 1000
min_audit_count = 20
```

Integrity score:

```text
chunk_integrity_rate = valid_chunk_responses / chunk_integrity_audits
object_integrity_rate = valid_object_responses / object_integrity_audits
I = 0.3 * chunk_integrity_rate + 0.7 * object_integrity_rate
```

Provider score:

```text
if I < 0.98: S = 0
else: S = A^1.0 * I^2.0
```

Audits target only active provider claims in v1. If a provider claims to provide an object/chunk, it can be audited for that claim.

Failed audits:

- lower provider score
- mark the local route/provider as suspect for that key
- create slashable evidence for future chain settlement

Auditor policy:

- anyone can locally audit for local routing
- settlement/reward-impacting audit results require trusted/staked auditors
- v1 mock config contains trusted auditor identities
- later this is replaced by chain-backed validator/auditor selection

Audit target selection:

- deterministic seeded sampling in v1
- chain randomness later

## 15. PoS and settlement

The PoS blockchain is not built before DHT infrastructure, but the architecture reserves ports for it.

Settlement responsibilities:

- stake registry
- protocol parameter source
- audit attestation intake
- provider scoring
- reward calculation
- slashing/finality later

V1 uses local/mock adapters behind:

- `ProtocolParamsPort`
- `StakeRegistryPort`
- `SettlementPort`

## 16. Public network and privacy

HIVEMIND v1 is public-by-design.

There is no payload encryption in v1. Users and agents must not publish secrets.

Future work may add:

- encrypted payloads
- private swarms
- access policies
- private namespaces

## 17. Observability and errors

Logging/tracing:

```text
tracing + tracing-subscriber
```

Example:

```bash
RUST_LOG=hivemind=debug,libp2p=info
```

Error handling:

```text
thiserror per library crate
anyhow only in binary/application composition
```

## 18. Test strategy

Test layers:

- core unit tests
- application tests with mocked ports
- adapter integration tests
- deterministic local multi-node smoke tests

Important tests:

- deterministic object ID generation
- deterministic chunk ID generation
- CBOR canonical encoding stability
- signature validation
- tag normalization
- filesystem atomic store behavior
- SQLite migrations/repositories
- libp2p request-response transfer
- Kademlia provider discovery
- two-node publish/fetch/tag smoke test

## 19. First vertical-slice definition of done

The first implementation milestone is complete when two local nodes can do all of this:

1. Node A starts with persisted network and agent keys.
2. Node B starts with persisted network and agent keys.
3. Nodes discover/connect via configured bootstrap or mDNS.
4. Node A receives `POST /v1/objects` with inline payload ≤16 KiB.
5. Node A receives `POST /v1/objects` with chunked payload >16 KiB.
6. Node A stores canonical object bodies and chunks.
7. Node A publishes DHT provider records for object IDs.
8. Node A publishes DHT provider records for every chunk ID.
9. Node A provides tag-bucket records for locally hosted object tags.
10. Node B can fetch an object by object ID through DHT provider discovery.
11. Node B resolves chunk providers via DHT per chunk ID.
12. Node B fetches chunks via batch request-response.
13. Node B verifies object ID, author signature, chunk hashes and assembled payload.
14. Node B can find candidate objects by exact tag lookup.
15. Node B validates tag candidates by fetching and verifying the object body.
16. `GET /health` returns liveness.
17. All behavior is covered by deterministic automated tests or local smoke tests.

## 20. GitHub Pages

The protocol website source lives in the `www/` directory.

Because GitHub Pages does not support `www/` as a branch publishing folder directly, deployment uses GitHub Actions:

```text
.github/workflows/pages.yml
```

The workflow uploads `www/` as the Pages artifact. The landing page is `www/index.html`. This architecture document remains `docs/architecture-v1.md`.
