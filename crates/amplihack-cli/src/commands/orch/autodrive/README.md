# Auto-drive authorization and state

This module implements local workflow safeguards for issues #2329 and #2575.
It never contacts a hosting service, decides readiness, or performs a merge.
The merge-ready agent assesses the PR and writes it up as a PR comment;
nothing here parses that write-up. The merge is requested by the bundled
`autodrive_merge.sh` under the conditions in
`docs/reference/autodrive-merge-ready-round.md`.

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
  `autodrive_state_dir must be an absolute path`. Without one, the runner uses
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
- `report`: build the run's `autodrive_outcome` (`schema_version` 2). It
  never fails and never panics. `merge` and `stop_reason` come only from
  tool-written JSON (`finalize_merge`, `merge_loop_result`); `summary` is the
  finalization agent's statement verbatim, cut on a character boundary at
  `MAX_RECORD_BYTES`. `already_merged` keeps its binding, selection,
  `run_id`, provenance and PR checks; its errors become a fixed `note`
  that copies no file contents. State is opened only from the authorization's
  bound state directory, compared lexically; nothing is joined onto a working
  directory or canonicalised.
- `helper`: local CLI operations under `amplihack orch helper autodrive`:
  the `State` and `Worktree` operations, and `authorization --invocation F
  --directory D`, which reads the captured invocation, checks the bound state
  directory and prints `{"allows_merge":bool,"run_id":...}`, exiting `2` on
  any error.

The parsed readiness record, its strict validation, the finalization observation,
the loop receipt and the `records`, `validation`, `completion`, `outcome`,
`review` and `followups` modules were retired by #2575. Crusty completion is
decided by `orch/autodrive_completion/`, unchanged.

`tests.rs` exercises authorization, private state, state-directory binding,
retired environment entries, `already_merged` and the assigned state
directory. `report_v2_tests.rs` covers any finalization text, the
`stop_reason` mapping and an oversized multi-byte summary. The CLI suite
`issue_2575_agentic_merge_ready.rs` runs the merge-ready round, loop endings,
merge tool and hand-off against a stub `amplihack` and `gh`, with `HOME`,
`AMPLIHACK_HOME`, the state directory and `CLAUDE_CONFIG_DIR` in temporary
directories. `issue_2329_autodrive_outcomes.rs` exercises runner boundaries,
including `--fresh` and recovery resume with stored authority and the
absolute-path refusal. Neither suite runs a real agent or merge, or
establishes live recovery or macOS execution.

Private state and prompts are not an OS security boundary against a process
holding the same user's credentials. Repository protections remain required.
