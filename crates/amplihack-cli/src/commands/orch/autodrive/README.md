# Portable auto-drive records

This module implements local workflow safeguards for issue #2329. It never
contacts a hosting service, decides semantic readiness, or performs a merge.
Agents observe and assess facts under trusted merge-ready policy.

- `authorization`: strictly capture explicit invocation inputs before expanding
  defaults. Local recipe defaults cannot select a PR or grant action authority.
  Context defaults are expanded only from the pinned trusted entrypoint.
  The full composer defaults to merge authorization; direct consumers and
  light mode default to verification. Dry-run suppresses follow-ups as well.
  Resume captures fresh authority; no candidate or inherited marker grants it.
  This includes `recipe run --fresh` and durable recovery resume: capture reads
  the raw explicit inputs before recovery normalisation, and every invocation
  gets a new `run_id`. After recovery admission the runner rewrites
  `no_merge`, `autodrive_dry_run`, `autodrive_authorization`,
  `autodrive_invocation_file` and `autodrive_state_dir` in the child context,
  so no stored or recovered value survives. The recovery fingerprint records
  only whether authority applies.
  `InvocationAuthorization` also binds the run's state directory
  (`bind_state_dir` / `state_dir`). An explicit `autodrive_state_dir` must be
  non-empty, absolute, free of `..` components and free of control
  characters; otherwise capture fails with
  `autodrive_state_dir must be an absolute path`. It must also be `0700`
  unless it sits directly under the auto-drive state root
  (`${AMPLIHACK_STATE_DIR:-${HOME:-/tmp}/.amplihack/state}/auto-drive`, the
  root `autodrive_state.sh` uses) and that root is private and owned by this
  user: only such slug folders, or a folder `EvidenceStore::create` has just
  created, have their group and other bits removed automatically. A file
  this user owns, read from a private state folder (`0700`, owned by this
  user), is also repaired to `0600` wherever the folder is, because no other
  user can create or change entries in such a folder. The
  folder's name plays no part (#2546, see
  `docs/reference/auto-drive-state-permissions.md`). Without one, the runner uses
  `<durable run folder>/autodrive-state`, or `autodrive-state` in the launch
  bundle for dry runs. The bound path is the absolute path passed to
  `EvidenceStore::create`, before canonicalisation. Round history and budgets
  are shared across drives only when they pass the same explicit directory.
- `trusted`: pin and privately copy launch recipes, skills, templates, helpers
  and evaluator; freeze authorization in child prompts and literal recipe
  contexts. Clean exact retired environment entries without sourcing them.
- `records` / `validation`: closed schema-1 identity, criterion, attempt,
  recovery, finalization and completion records. Exactly 13 criteria, current
  head/base/run binding, concrete observations and no contradictions establish
  structural readiness. Semantic correctness remains the agent's obligation.
  `null_as_unknown` reads a `null` or missing value in the listed string
  fields (every `Binding` field, `Attempt.cause` and `.operation`,
  `reviewed_head_sha`, `verified_head_sha`, every `ReviewRange` field,
  `Evidence.source` and `.observation`, `Recovery.interface` and `.evidence`,
  and the `FinalizationObservation` strings) as `""`. Every other field stays
  strict, `deny_unknown_fields` stays on every type, and serialisation is
  unchanged. `validation::unknown_fields` reports each empty or `"unknown"`
  value as a sorted `unknown-field:<path>` blocker. Paths use array positions
  and only a criterion id found in the fixed criteria list. An empty cause on
  a succeeded attempt is not reported.
- `state`: create private directories and files, reject unsafe historical
  modes, links, traversal and special files; bound reads; validate and hash
  opened bytes; publish new snapshots atomically without replacement.
- `outcome`: keep readiness, requested action, observed state and stop reason
  separate. Merge-ready completion needs both an accepted `MERGE_READY`
  round and evaluator `DONE`. Crusty completion is not reduced here; it is
  decided by `orch/autodrive_completion/` (an evidence-valid `CLEAN` or
  `MINOR_ONLY` completes without an evaluator call). Merge
  success additionally requires authorized expected-head protection and
  platform confirmation; queue admission or exit zero is insufficient.
- `review`: verify the established post-review allowlist against local Git.
- `completion`: seal accepted round bytes only after the merge-ready loop
  observes an accepted `MERGE_READY` round and evaluator `DONE`; reject
  receipts from an older invocation.
- `followups`: validate private MINOR_ONLY issue mappings for reporting.
- `qa_record` (`helper autodrive qa-record --inputs --repo --out [--round]`):
  the only writer of `qa-evidence.json` for criterion 1 (#2569). It reads
  the agent's judgement from stdin (at most 1 MiB, unknown fields refused,
  every field required), applies the record's shape rules and type limits,
  takes `head_sha` from trusted Git, derives `qa_reason`, copies
  `declared_qa` and the flat legacy fields from `qa-inputs.json` without
  `log_tail`, and writes the file atomically with mode `0600`. Exit 0 prints
  the bytes written; 2 is a refused judgement; 1 is an environment error.
  It judges nothing and never opens cited paths. These checks are the
  record's type contract and replace the shell status mapping that
  `autodrive_qa_evidence.sh` used to apply. See
  [Auto-drive QA](../../../../../../docs/reference/auto-drive-qa.md#the-qa-record-helper).
- `report`: replay the same complete reduction boundary as `helper reduce`,
  including the pre-action accepted snapshot, local Git/review coverage, artifact
  bytes and loop receipt. Require a matching private outcome snapshot from
  successful reduction; retain concrete rejection causes in incomplete reports.
  The evidence store is opened only from the authorization's bound state
  directory; an unset binding is incomplete. The child's `autodrive_state_dir`
  and the reducer's `state_dir` must equal the bound path lexically (empty
  counts as absent; any other string or a non-string value is a mismatch).
  Nothing is joined onto `working_dir` or canonicalised. A mismatch reports
  `incomplete` with a fixed cause; paths in it are `{:?}`-escaped.
- `helper`: optional local CLI operations under `amplihack orch helper
  autodrive`. Required record validation in the recipe uses these local
  operations; platform access and recovery have no prescribed interface.
  `assess` and `round` read stdin (at most 4 MiB) only after authorisation
  and the state directory are established and, for `round`, the record's
  parent is confirmed to be the state directory. They then run parse,
  `unknown_fields`, `binding_drift` and the integrity checks in that order.
  `binding_drift` is the public wrapper over the private `drift()`, which
  also returns `current_head` and `current_base`. It validates the binding
  and both branch names, and checks that the head branch matches the
  worktree branch, before any Git command reads `HEAD`; it returns
  `stale-evidence-head`, `stale-evidence-base` or
  `stale-evidence-merge-base`; the merge-base is compared only when the head
  and base match. If `git rev-parse --verify HEAD` fails (for example in a
  repository with no commits), the read adds the fixed blocker
  `current-head-unreadable`, sets `current_head` to `"unknown"` and
  `current_base` to `""`, and returns before any comparison; this applies on
  the parse-error, unknown-field and drift paths alike, and the Git error
  goes only to `tracing::debug!`. An invalid binding, an invalid branch
  name, a branch mismatch, grafts and every Git failure other than reading
  `HEAD` remain hard errors, as do `validate_candidate` errors (for example
  an incomplete criterion with no cause; see #2539). A parse, unknown-field,
  unreadable-head or drift fault is not a hard error:
  - `assess` prints `{"status":"refresh_required","accepted":null,
    "readiness":"incomplete","refresh_required":true,"evidence_head",
    "current_head","current_base","blockers"}` and exits 0. It writes no
    snapshot, reads no follow-ups and leaves `--directory` unchanged. Success
    adds `"status":"accepted"` and `"refresh_required":false`.
  - `round` writes a `NOT_MERGE_READY` record (`test_signal` `BLOCKED`,
    `ci_signal` `UNVERIFIED`, `accepted_evidence` null) with its blockers,
    writes the same blockers one per line to `<record>.findings`, prints the
    record and exits 0. Every round record has the same keys, including
    `blockers`, and `blocker_count` equals their number.
  Blocker text is built from fixed names, positions and serde's error class,
  line and column, never from record values. Each blocker is at most 256
  bytes with no control characters. Echoed SHAs must be 40 or 64 hexadecimal
  characters and `round_label` must match `[A-Za-z0-9._-]{1,128}`; otherwise
  they are written as `""`; this applies to `evidence_head`, `current_head`
  and `current_base`, so on the unreadable-head path both `current_head` and
  `current_base` are printed as `""`. In a repository with no commits the
  branch check still passes when the binding names the unborn current
  branch, so the result is `current-head-unreadable`, not a branch-mismatch
  hard error.
  `local_binding` keeps its three strict messages, also rejects any other
  drift blocker (including `current-head-unreadable`), and
  `validate_reduction` still calls it, so stale evidence or an unreadable
  worktree `HEAD` never passes reduction. The strict check
  runs after the merge action, so a merge is never reduced against a head
  that has moved since acceptance.

The full serialized agent contract is in the bundled skill's
`evidence-contract.md`. `tests.rs` exercises authorization, stale/mismatched
records, concrete evidence, failure/recovery, private state and outcomes,
including state directory mismatches and a resumed run that tries to replay an
earlier run's finalisation or receipt. The CLI integration suite
(`issue_2329_autodrive_outcomes.rs`) exercises runner boundaries without
executing a real agent or merge, including `--fresh` and recovery resume with
stored authority and the absolute-path refusal. Its runner stubs report
`durable_recovery_v1`. The stubs do not write a real recovery descriptor, so
the recovery case reruns with `--fresh` and a `-c` context that carries stored
authority; descriptor-driven resume itself is not exercised. Neither suite establishes live recovery or macOS execution.

Private state and prompts are not an OS security boundary against a process
holding the same user's credentials. Repository protections remain required.
