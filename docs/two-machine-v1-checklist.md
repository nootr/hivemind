# Two-machine v1 checklist

Use this before calling a release v1-ready on a real LAN.

## Machine A and B

Install, initialize and start one node per machine:

```bash
curl -fsSL https://hivemind.jhx.app/install.sh | sh
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

## Discover

Keep both nodes running on the same LAN for a few seconds, then check discovered candidates on both machines:

```bash
hive peers
```

If UDP discovery is blocked, run diagnostics and use scan or manual join as a fallback, then check peers again:

```bash
hive doctor
hive scan <your-lan-cidr>
hive join <other-machine-advertised-node-url>
hive peers
```

Discovery sends both limited broadcast (`255.255.255.255:7748`) and interface-directed broadcasts such as `10.0.1.255:7748`. If discovery fails on a real LAN, check local firewall settings, Wi-Fi/AP client isolation and whether UDP port `7748` is allowed.

Expected:

- each side lists the other as `unknown`;
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

## Agent presence

On each machine with an active agent session:

```bash
hive watch --agent <agent-name> --capabilities rust,review
```

Or, if you do not want the foreground watcher, refresh presence manually:

```bash
hive agent heartbeat --name <agent-name> --capabilities rust,review
hive agents
```

Expected: trusted peers can see active/stale agent heartbeats with `hive agents`.

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
