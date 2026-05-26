# Two-machine v1 checklist

Use this before calling a release v1-ready on a real LAN.

## Machine A and B

Install, initialize and start one node per machine:

```bash
curl -fsSL https://raw.githubusercontent.com/nootr/hivemind/main/install.sh | sh
hive node init
hive node start
hive node status
hive setup
```

Record for each machine:

- advertised node URL
- optional node name/hostname
- full node ID

Names and URLs are only recognition hints. Verify node IDs out-of-band before trust.

## Join

From machine A:

```bash
hive join <machine-b-advertised-node-url>
hive peers
```

From machine B:

```bash
hive join <machine-a-advertised-node-url>
hive peers
```

Expected:

- each side lists the other as `untrusted`;
- peer name may help identify the machine;
- full node ID is visible;
- no chat is delivered before trust.

## Trust

After comparing full node IDs out-of-band:

```bash
hive peer trust <other-node-id>
hive peers
```

Run on both machines. Trust is local and must be mutual for reliable two-way chat.

## Chat

Machine A:

```bash
hive say "hello from machine A"
```

Machine B:

```bash
hive chat
hive say "hello from machine B"
```

Machine A:

```bash
hive chat
```

Expected: both signed messages appear with trusted authors.

## Restart and network change

On each machine:

```bash
hive node status
hive setup
```

If a machine's LAN IP changes, the advertised node URL should update when `public_url` is omitted from `~/.hivemind/node.toml`. If detection is wrong, override explicitly:

```bash
hive node init --public-url http://<reachable-ip>:7747 --force
hive node start
hive node status
```
