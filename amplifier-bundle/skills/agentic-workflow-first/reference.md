# Anti-Patterns & Typed-Record Replacements

Four anti-patterns drawn from recurring agentic-workflow history. Each is
paired with the pattern that replaces it.

## AP-1 — Brittle stdout / JSON scraping

**Symptom:** a step recovers data by parsing another step's printed prose or by
scraping JSON out of an agent's free-text output (`extract_and_parse_json`,
`from_recipe_envelope`). Fragile to formatting drift; fails open on partial output.

**Reject because:** the contract is implicit and unvalidated; a reworded agent reply
silently breaks the handoff, and a partial parse can proceed on default/garbage data.

**Replace with — the typed-record handoff (THE way data flows agent→agent / agent→rail):**
- The producing step writes a **typed record**: a small file with an explicit schema
  (versioned, with a nonce/timestamp).
- Create it **owner-only** (`0o600`) **atomically at creation** (open with mode; never
  `chmod` after write — that is a TOCTOU window).
- The consuming step reads it for **record integrity**: missing file, wrong permissions,
  parse failure, schema mismatch, or a stale record → refuse to act on it, and hand the
  failure to an agent step with the reader's own reason as evidence, so a replayed nonce
  can be told from a missing file. Never proceed on partial or default data.
- Include a **freshness / nonce** check so a stale record from a previous run cannot be
  replayed by a concurrent run.

## AP-2 — Imperative heuristics standing in for judgment

**Symptom:** a Rust function (`orient`, `classify_signal`, `decide`, `prioritize`)
hardcodes priority/classification logic that is really a judgment call.

**Reject because:** judgment encoded as branches rots, cannot explain itself, and
cannot adapt to phrasing it did not anticipate.

**Replace with:** an **agentic step** (Step 3a) behind a prompt. The prompt states the
classes/priorities and the decision rules; the agent classifies and emits a typed
record. The rail only routes on the record's typed field.

## AP-3 — Wall-clock timeouts that kill working agentic steps

**Symptom:** a per-step wall-clock `timeout:` aborts an agentic step mid-thought.

**Reject because:** agentic steps have highly variable, non-linear runtimes; a wall
clock kills correct in-progress work. (This repo's workflows adopt a NO-TIMEOUT policy
for agent steps for exactly this reason.)

**Replace with:** **idle / liveness detection** — abort only on lack of progress
(no output / no heartbeat), not on elapsed wall-clock time. Shell `timeout` may still
guard *external network commands* (`gh`, `curl`, `git remote`) inside bash tool steps,
because those are not mid-thought abort gates.

## AP-4 — Large payloads via argv / env (E2BIG)

**Symptom:** passing a big blob (a diff, a corpus, a record) as a command-line argument
or environment variable. Hits the OS `E2BIG` limit and leaks contents through
`ps` / `/proc/<pid>/environ`.

**Reject because:** it breaks on size AND exposes payloads/secrets to any process that
can read the process table.

**Replace with:** route the payload via **stdin** or a **file the tool reads**. The
producing step writes the file (owner-only if sensitive); the consuming tool reads it
by path or from stdin. argv/env carry only small, non-secret identifiers.
