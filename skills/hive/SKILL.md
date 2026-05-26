---
name: hive
description: Team shared memory for AI agents using the HIVEMIND hive CLI. Use when a task may benefit from remembered team/project facts, runbooks, decisions, procedures, reusable skills, or when new durable learning should be saved for future agents in the same workspace.
---

# Hive

Use HIVEMIND as shared team memory for AI agents.

The user-facing concept is `/hive`. The concrete tool for this skill is the `hive` CLI.

## Setup check

Before first use in a workspace, verify the CLI is available and run guided setup:

```bash
hive --help
hive setup
```

If `hive setup` reports no local config and the user wants this agent to start the first/local node, guide or perform these steps:

1. Ensure a node config exists. Prefer an existing `node.toml`; otherwise use `examples/local-node.toml` as the template.
2. For colleague/LAN discovery, the node must listen on a reachable address, for example:
   ```toml
   [api]
   bind_addr = "0.0.0.0:7747"
   auth_token_file = "./data/api.token"
   ```
   Keep `127.0.0.1:7747` only for same-machine tests.
3. Start the node and keep it running:
   ```bash
   cargo run -p hivemind-node -- --config node.toml
   ```
4. In another shell/session, configure the CLI with the local admin token file:
   ```bash
   hive init --node-url http://127.0.0.1:7747 --token-file ./data/api.token
   ```
5. Run `hive setup` again. It should discover local/team nodes and store discovered nodes as untrusted peer candidates when config exists.

Temporary environment variables are also supported:

```bash
HIVEMIND_NODE_URL
HIVEMIND_API_TOKEN
```

If setup cannot be completed, explain exactly what is missing. Stop; do not invent memory results.

### Peer trust

`hive setup` / `hive discover` may find local Hive nodes through active discovery or node beacons and store peer node IDs/public-key fingerprints as untrusted candidates. Discovery is not trust and does not grant access. Never trust peers automatically. Show the user the discovered node URL and node ID/fingerprint, ask them to compare it with their teammate out-of-band, and only run `hive peer trust <node-id>` after explicit approval.

### Register that Hive is available

After setup succeeds, save a small bootstrap memory so future agents can discover that this workspace uses Hive team memory:

```bash
hive remember "This workspace has HIVEMIND /hive team memory configured. Before substantive work, search Hive for relevant project facts, runbooks, decisions, procedures, and reusable skills. Save durable new learnings back to Hive when useful." \
  --tag hive \
  --tag setup \
  --tag team-memory
```

Do not store API tokens, secrets, credentials, private keys, customer secrets, or private personal data in this bootstrap memory.

## Default behavior

For any non-trivial task, silently consider whether team memory could help. Prefer checking Hive when the task involves:

- an existing project, repository, customer, service, workflow, or incident
- recurring procedures, runbooks, deployment steps, debugging paths, or conventions
- team decisions or constraints that may have been learned before
- requests like “remember”, “save this”, “next time”, “we learned”, or “use hive”

Do not ask the user whether to check memory unless the lookup could reveal sensitive information or the user asked you not to use memory.

## Read memory before acting

Choose 1-3 specific exact tags from the task. Use lowercase, short tags such as repo name, service name, domain, tool, language, or workflow.

```bash
hive find <tag>
```

If useful memories are found, retrieve the most relevant ones:

```bash
hive use <object_id>
```

Use retrieved memory as team context, but still reason critically. If memory conflicts with the current repository, user instruction, or observed facts, prefer the current evidence and mention the conflict briefly.

## Save durable learnings

Save memory when a task produces reusable team knowledge that future agents should know, such as:

- project conventions or architecture decisions
- confirmed fixes and debugging procedures
- runbooks and operational steps
- integration details that are likely to recur
- reusable agent workflows or skills

Use concise, standalone text. Include enough context that a future agent can apply it without reading this conversation.

```bash
hive remember "<standalone memory text>" --tag <tag> --tag <tag>
```

Prefer 2-5 tags. Include at least one domain/project tag and one type tag when possible:

- `runbook`
- `decision`
- `fact`
- `procedure`
- `debugging`
- `convention`
- `skill`

## Do not save

Do not save:

- secrets, tokens, passwords, private keys, session cookies, or credentials
- sensitive personal data unless the user explicitly requests it and it is appropriate
- private customer data unless the team policy explicitly allows it
- transient status updates that will be stale soon
- guesses, unverified assumptions, or speculative conclusions
- large raw logs or generated noise

If a useful learning contains sensitive details, save a sanitized version.

## Reporting style

Keep memory use low-noise:

- If Hive memory materially affects the answer, mention it briefly.
- If no relevant memory is found, do not dwell on it.
- When saving memory, say what was saved in one short sentence.
- Do not expose internal tokens, node URLs, or raw API errors unless needed for troubleshooting.

## Failure handling

If `hive find`, `hive use`, or `hive remember` fails:

1. Continue the task without Hive if possible.
2. Mention the failure briefly only if it affects confidence or expected behavior.
3. Do not retry repeatedly.
4. Never fabricate a memory lookup result.
