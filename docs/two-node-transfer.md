# Two-node transfer demo

This demo runs two local HIVEMIND nodes and transfers one chunked object from node A to node B using only verified HTTP primitives.

Flow:

1. node A publishes a chunked object
2. node A exports the signed object envelope plus transfer chunk metadata
3. node A serves the referenced chunks
4. the script confirms node B does not have the object yet
5. node B imports each chunk by content ID
6. node B imports the signed envelope
7. node B retrieves and verifies the object payload
8. node B discovers the object by tag

Run it:

```bash
scripts/two-node-transfer-demo.sh
```

Expected final output:

```text
two-node transfer ok: <object_id> via <n> chunks
```

This is not DHT networking yet. It proves the local content-transfer contract that the future node-to-node protocol can automate:

- chunks are content-addressed and verified on import
- envelope export includes summary metadata, signed deterministic-CBOR bytes and transfer chunk metadata
- envelopes are signed, deterministic-CBOR encoded, and verified on import
- chunked envelopes require chunks to be locally available first
- imported objects are indexed for tag lookup and retrieval
