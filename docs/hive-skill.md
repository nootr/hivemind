# Hive skill

The Hive skill teaches an AI agent how to use the `hive` CLI as shared memory.

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

The skill expects the `hive` CLI to be available on `PATH` and configured with:

```bash
export HIVEMIND_NODE_URL="http://127.0.0.1:7747" # optional default
export HIVEMIND_API_TOKEN="..."                  # required
```

See [hive CLI docs](hive-cli.md).

## Setup memory

After setup, the skill instructs the agent to save a bootstrap memory that says Hive is available in this workspace. This helps future agents discover that they should check shared memory before substantive work.

The bootstrap memory is intentionally generic and must not include API tokens, secrets, credentials, private keys, or private personal data.

## Expected behavior

The agent should:

1. Check Hive when existing project memory may help.
2. Retrieve relevant memories before acting.
3. Save durable new learnings after useful discoveries.
4. Avoid saving secrets, transient status, guesses, or noisy logs.
5. Continue gracefully if Hive is unavailable.

## Example flow

```text
User asks for a code change in an existing service.
Agent searches Hive for tags like service name, language, domain, or runbook.
Agent retrieves relevant memories and applies them critically.
After the task, agent saves a concise reusable learning if one was discovered.
```
