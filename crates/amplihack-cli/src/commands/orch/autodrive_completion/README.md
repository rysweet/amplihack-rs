# Crusty completion dispatch

The final Crusty agent decides whether the current review objective is complete
before convergence evaluation. This module validates that decision and either
returns objective completion or launches the trusted evaluator. Follow-up filing
and phase completion remain separate driver operations.

The public CLI is `amplihack orch helper autodrive-completion-{prepare,seal,write,dispatch}`:

- `prepare` takes the state directory, round label, designated record, PR worktree,
  review directory and launch tools. Evaluator data arrives through an allowlisted
  JSON file or `--context-from-file key=path`. It returns a fresh `run_id`, a
  `private_state` path and a unique `decision_file` path.
- `seal` captures the record and its sidecars in driver-owned state after the
  mechanical record-writing step, before the decision agent runs. Preparing an
  existing record seals it immediately.
- `write` accepts only `complete` or `evaluate` and a single-line reason of
  1–1024 bytes. It computes the record hash and atomically creates the six-field
  version-1 decision. It cannot replace an existing output.
- `dispatch` takes driver-owned state and the child's actual exit status. It
  independently validates identity, the manifest-bound record, latest copy,
  sidecars, review-directory stamp and current head. Valid successful `CLEAN` or
  `MINOR_ONLY` completion returns `DONE`, `source=objective-completion` and
  `evaluator_invoked=false`. An atomic receipt prevents repeat dispatch.
- The completion claim also compares the PR head on GitHub with the reviewed
  head, through `autodrive_gh` (GraphQL, then REST, then `git ls-remote`; issue
  #2333). The PR number comes from `prepare --pr`, else from the branch's open
  PR. A different head fails closed as before. A head refused on every path, or a branch
  with no open PR, is an error. A head unavailable on every path returns
  `WAITING` with exit 76: no receipt, no evaluator, the sealed evidence
  unchanged, so the same round dispatches again once a read answers.

`evaluate` requires an unresolved review. The evaluator runs with typed process
arguments, file-backed context, the captured sibling depth and inherited policy
ceiling. Its log parser follows the runner formatter contract; terminal refusal,
usage-limit and orphaning classifications retain their meanings. An evaluator
`DONE` cannot itself certify an unresolved objective.

The Unix private-file layer uses descriptor-relative no-follow access and checks
ownership, permissions, bounds and file type. Same-user arbitrary code execution
is outside this integrity boundary. Other platforms fail explicitly.

The full contract is in `docs/reference/autodrive-crusty-completion.md`.
Regression tests execute the extracted YAML and production helpers with only
agent and GitHub boundaries stubbed. A separate ignored live-agent test executes
the actual next-action prompt for controlled delivery validation.
