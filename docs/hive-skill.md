# Hive skill

The Hive skill teaches an AI agent how to use the `hive` CLI as shared memory for a team or workspace.

User-facing name:

```text
/hive
```

Concrete tool used by the skill:

```bash
hive
```

## Install location

For Pi, project skills can live in:

```text
.pi/skills/
.agents/skills/
skills/
```

This repository ships the skill at:

```text
skills/hive/SKILL.md
```

Depending on the agent runtime, copy or reference that directory in the runtime's skill search path.

## Prerequisites

The skill expects the `hive` CLI to be available on `PATH`. Agents should start setup with:

```bash
hive --help
hive setup
```

If no config exists and the user wants this agent to start the first/local node, the agent should use `node.toml` or `examples/local-node.toml`, ensure the node binds to a reachable address for LAN discovery (`0.0.0.0:7747` instead of `127.0.0.1:7747`), start `hivemind-node`, then configure the CLI:

```bash
hive init --node-url http://127.0.0.1:7747 --token-file ./data/api.token
```

Environment variables are also supported for temporary setup:

```bash
export HIVEMIND_NODE_URL="http://127.0.0.1:7747"
export HIVEMIND_API_TOKEN="..."
```

See [hive CLI docs](hive-cli.md).

## Setup memory

After setup, the skill instructs the agent to save a bootstrap memory that says Hive is available for this team/workspace. This helps future agents discover that they should check team memory before substantive work.

The bootstrap memory is intentionally generic and must not include API tokens, secrets, credentials, private keys, customer secrets or private personal data.

## Expected behavior

The agent should:

1. Check Hive when existing team/project memory may help.
2. Retrieve relevant memories before acting.
3. Save durable new learnings after useful discoveries.
4. Avoid saving secrets, transient status, guesses or noisy logs.
5. Continue gracefully if Hive is unavailable.
6. Run `hive setup` for guided discovery when asked to configure Hive.
7. Show discovered node IDs/fingerprints and ask the user before trusting any peer candidate from `hive peers`.

## Example flow

```text
User asks for a code change in an existing service.
Agent searches Hive for tags like service name, language, domain or runbook.
Agent retrieves relevant team memories and applies them critically.
After the task, agent saves a concise reusable learning if one was discovered.
```
