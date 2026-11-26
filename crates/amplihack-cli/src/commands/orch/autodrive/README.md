# Auto-drive authorization and state

This module implements local workflow safeguards for issues #2329 and #2575.
It never contacts a hosting service, decides readiness, or performs a merge.
The goal loop's agent judges readiness each turn from a fresh reading;
nothing here parses its turn records. The merge is requested by the loop's
merge action, the bundled `autodrive_merge.sh`, under the conditions in
`docs/reference/autodrive-goal-loop.md`.

- `authorization`: strictly capture explicit invocation inputs before expanding
  defaults. Local recipe defaults cannot select a PR or grant action authority.
  Context defaults are expanded only from the pinned trusted entrypoint.
  The full composer defaults to merge authorization; direct consumers and
  light mode default to verification. Dry-run suppresses follow-ups as well.
  Resume captures fresh authority; no agent output or inherited marker grants it.
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
- `identity`: `Binding`, `validate_binding` and `MAX_RECORD_BYTES`. `Binding`
  keeps `deny_unknown_fields` because the runner, not an agent, produces it.
  Used by `authorization` and `report`; `state` uses `MAX_RECORD_BYTES`.
- `state`: create private directories, reject unsafe historical modes,
  links, traversal and special files, and read private files with a size
  bound (`EvidenceStore::create`, `read_private`).
- `private_mode`: the one permission rule `state` and the completion checker
  share (#2546). `access` decides; `make_private` removes extra group and
  other bits with `fchmod` on the checked descriptor and logs each repair.
  World-writable entries, other owners or groups, symlinks and hard links
  stay refused.
- `report`: build the run's `autodrive_outcome` (`schema_version` 2) for the
  goal loop (#3129). It never fails, never panics and makes no network call.
  It reads two records, only from the authorization's bound state directory
  and only through `EvidenceStore::read_private`: `loop-end.json`, which only
  the loop driver and its end actions write, and `merge-result.json`, the
  loop's merge action's result. Each must be a regular, private file within
  `MAX_RECORD_BYTES`, holding one JSON object whose `run_id` is this run's.
  If the context's `autodrive_state_dir` is not the bound directory, compared
  lexically, neither is read. The `end` word alone decides `stop_reason`:
  `merged` is `completed`; `ready` is `completed` only when the captured
  invocation does not allow merging, and `handoff` when it does; `handoff`,
  `closed`, `stopped`, `failed`, `usage_limit` and `orphaned` keep their
  names; `refused` is `terminal_refusal`; an unknown word or no record is
  `failed` with a `note`. `merge-result.json` only fills `merge`, and a
  `merged` end without this run's merged result gets a note that the merge
  was not by this run's merge action. `loop_end` echoes the end word,
  `after_merge_steps` lists a merged end's later steps (at most 32 strings,
  8 KiB in all), and `loop_result` is always null. `summary` is the
  `autodrive_finalization` context value verbatim, cut on a character
  boundary at `MAX_RECORD_BYTES`. `already_merged` keeps its binding,
  selection, `run_id`, provenance and PR checks and is still `completed`.
  Notes copy no file contents. Turn records (`turns/NNN/record.md`) are never
  read. See `docs/reference/autodrive-goal-loop.md#reported-outcomes`.
- `helper`: local CLI operations under `amplihack orch helper autodrive`:
  the `State` and `Worktree` operations, and `authorization --invocation F
  --directory D [--pr N]`, which reads the captured invocation, checks the
  bound state directory and prints `{"allows_merge":bool,"no_merge":bool,
  "dry_run":bool,"run_id":...}`, plus `"pr_selected":bool` with `--pr`,
  exiting `2` on any error.

The parsed readiness record, its strict validation, the finalization observation,
the loop receipt and the `records`, `validation`, `completion`, `outcome`,
`review` and `followups` modules were retired by #2575. Crusty completion is
decided by `orch/autodrive_completion/`, unchanged.

`tests.rs` exercises authorization, private state, state-directory binding,
retired environment entries, `already_merged` and the assigned state
directory. `report_goal_tests.rs` holds the goal loop's report rules, and
`report_v2_tests.rs` covers any finalisation text, the end-word mapping,
records of another run or PR, and an oversized multi-byte summary. The CLI suite
`issue_2575_agentic_merge_ready.rs` runs the goal loop's merge action and its
trust boundaries against a stub `amplihack` and `gh`, with `HOME`,
`AMPLIHACK_HOME`, the state directory and `CLAUDE_CONFIG_DIR` in temporary
directories. `issue_2329_autodrive_outcomes.rs` exercises runner boundaries,
including `--fresh` and recovery resume with stored authority and the
absolute-path refusal. Neither suite runs a real agent or merge, or
establishes live recovery or macOS execution.

Private state and prompts are not an OS security boundary against a process
holding the same user's credentials. Repository protections remain required.
