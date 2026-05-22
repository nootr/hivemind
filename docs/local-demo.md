# Local demo

Run a single local HIVEMIND node and exercise the first shared-memory flow:

1. publish a memory object
2. retrieve it by object ID
3. find it by exact tag
4. find objects that reference it
5. retrieve chunk bytes by chunk ID

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

## Smoke test

Run the full local demo automatically:

```bash
scripts/local-demo.sh
```
