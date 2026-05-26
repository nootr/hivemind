---
name: hive
description: Shared memory for AI agents using the HIVEMIND hive CLI. Use when a task may benefit from remembered project facts, runbooks, decisions, procedures, reusable skills, or when new durable learning should be saved for future agents.
---

# Hive

Use HIVEMIND as shared memory for AI agents.

The user-facing concept is `/hive`. The concrete tool for this skill is the `hive` CLI.

## Setup check

Before first use in a workspace, verify the CLI is available and configured:

```bash
hive --help
```

The CLI needs:

```bash
HIVEMIND_NODE_URL   # optional, defaults to http://127.0.0.1:7747
HIVEMIND_API_TOKEN  # required
```

If the CLI is missing or not configured, explain exactly what is missing and stop. Do not invent memory results.

### Register that Hive is available

After setup succeeds, save a small bootstrap memory so future agents can discover that this workspace uses Hive:

```bash
hive remember "This workspace has HIVEMIND /hive shared memory configured. Before substantive work, search Hive for relevant project facts, runbooks, decisions, procedures, and reusable skills. Save durable new learnings back to Hive when useful." \
  --tag hive \
  --tag setup \
  --tag agent-memory
```

Do not store API tokens, secrets, credentials, private keys, or private personal data in this bootstrap memory.

## Default behavior

For any non-trivial task, silently consider whether shared memory could help. Prefer checking Hive when the task involves:

- an existing project, repository, customer, service, workflow, or incident
- recurring procedures, runbooks, deployment steps, debugging paths, or conventions
- decisions or constraints that may have been learned before
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

Use retrieved memory as context, but still reason critically. If memory conflicts with the current repository, user instruction, or observed facts, prefer the current evidence and mention the conflict briefly.

## Save durable learnings

Save memory when a task produces reusable knowledge that future agents should know, such as:

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
