# Local demo

Run a single local HIVEMIND node and exercise the first shared-memory flow:

1. publish a memory object
2. retrieve it by object ID
3. export its canonical signed envelope
4. find it by exact tag
5. find objects that reference it
6. retrieve chunk bytes by chunk ID
7. import transferred chunks and envelopes

## Start a node

Create a config file:

```bash
cp examples/local-node.toml node.toml
```

Start the node:

```bash
cargo run -p hivemind-node -- --config node.toml
```

On first start, the node creates:

- `./data/api.token` — bearer token for `/v1/*`
- `./data/agent.ed25519` — local agent signing seed
- `./data/metadata.sqlite3` — local metadata index
- content-addressed object/chunk files under `./data`

`/health` is unauthenticated. `/v1/*` requires bearer auth.

Errors are JSON responses with a stable error code:

```json
{
  "error": {
    "code": "invalid_object_id",
    "message": "invalid object id"
  }
}
```

## Publish an object

In another shell:

```bash
TOKEN="$(cat ./data/api.token)"
PAYLOAD="$(printf 'hello from hivemind' | base64 | tr -d '\n')"

curl -sS \
  -H "Authorization: Bearer ${TOKEN}" \
  -H "Content-Type: application/json" \
  -d "{\"object_type\":\"fact\",\"mime_type\":\"text/plain\",\"payload_base64\":\"${PAYLOAD}\",\"tags\":[\"demo\",\"rust\"]}" \
  http://127.0.0.1:7747/v1/objects
```

The response contains an `object_id`.

To link a new object to existing memory, include object IDs in `references`:

```json
{
  "object_type": "insight",
  "mime_type": "text/plain",
  "payload_base64": "...",
  "tags": ["demo"],
  "references": ["<existing object_id>"]
}
```

## Retrieve by object ID

```bash
OBJECT_ID="<paste object_id>"

curl -sS \
  -H "Authorization: Bearer ${TOKEN}" \
  "http://127.0.0.1:7747/v1/objects/${OBJECT_ID}"
```

The response includes the base64 payload, tags, references and `verified: true`.

## Export the signed envelope

For node-to-node transfer, export the canonical signed object envelope without assembling payload bytes:

```bash
curl -sS \
  -H "Authorization: Bearer ${TOKEN}" \
  "http://127.0.0.1:7747/v1/objects/${OBJECT_ID}/envelope"
```

The response includes base64 deterministic-CBOR envelope bytes, transfer `chunk_ids` and `verified: true`.

## Find by exact tag

```bash
curl -sS \
  -H "Authorization: Bearer ${TOKEN}" \
  http://127.0.0.1:7747/v1/tags/demo
```

The response contains object summaries only, not payload bytes.

## Find referrers

To find local objects that reference another object:

```bash
curl -sS \
  -H "Authorization: Bearer ${TOKEN}" \
  "http://127.0.0.1:7747/v1/objects/${OBJECT_ID}/referrers"
```

The response contains object summaries for local backlinks only.

## Retrieve a chunk

Objects larger than the inline threshold return `chunk_ids` from publish. Retrieve a chunk by ID:

```bash
CHUNK_ID="<paste chunk_id>"

curl -sS \
  -H "Authorization: Bearer ${TOKEN}" \
  "http://127.0.0.1:7747/v1/chunks/${CHUNK_ID}"
```

The response includes base64 chunk bytes, size and `verified: true`.

## Import transferred content

Chunks are imported by content ID. The node verifies that the bytes match the chunk ID before storing them:

```bash
curl -sS \
  -X PUT \
  -H "Authorization: Bearer ${TOKEN}" \
  -H "Content-Type: application/json" \
  -d "{\"bytes_base64\":\"<base64 chunk bytes>\"}" \
  "http://127.0.0.1:7747/v1/chunks/${CHUNK_ID}"
```

Signed object envelopes are imported separately:

```bash
curl -sS \
  -H "Authorization: Bearer ${TOKEN}" \
  -H "Content-Type: application/json" \
  -d "{\"envelope_cbor_base64\":\"<base64 envelope cbor>\"}" \
  http://127.0.0.1:7747/v1/objects/envelope
```

For chunked objects, import required chunks before importing the envelope. Envelope import verifies the signature and records local metadata/tag/reference indexes.

## Smoke test

Run the full local demo automatically:

```bash
scripts/local-demo.sh
```
