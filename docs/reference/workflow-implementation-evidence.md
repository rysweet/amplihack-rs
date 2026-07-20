# Evidence & Bookkeeping Gate Hardening Reference

> [Home](../index.md) > Reference > Evidence & Bookkeeping Gate Hardening

Canonical contract for how every evidence, bookkeeping, compliance, and
finalization "gate" in `default-workflow` and its sub-recipes resolves its
bundled shell helper and decides whether a failure is fatal or a visible,
non-fatal degrade.

This feature roots out the entire *class* of "evidence/bookkeeping gate"
brittleness tracked by issues
[#962](https://github.com/rysweet/amplihack-rs/issues/962) (root cause of
[#964](https://github.com/rysweet/amplihack-rs/issues/964)) and the prior
piecemeal fixes
[#757](https://github.com/rysweet/amplihack-rs/issues/757),
[#770](https://github.com/rysweet/amplihack-rs/issues/770),
[#777](https://github.com/rysweet/amplihack-rs/issues/777),
[#848](https://github.com/rysweet/amplihack-rs/issues/848),
[#852](https://github.com/rysweet/amplihack-rs/issues/852). It extends the
git-identity fallback ladder landed for
[#955](https://github.com/rysweet/amplihack-rs/issues/955) to every bundled
helper site.

Updated: 2026-07-19

## Contents

- [Problem this solves](#problem-this-solves)
- [The two invariants](#the-two-invariants)
- [Canonical resolution ladder](#canonical-resolution-ladder)
- [Terminal-action policy: FATAL vs DEGRADE](#terminal-action-policy-fatal-vs-degrade)
- [The FATAL allowlist](#the-fatal-allowlist)
- [Gate inventory](#gate-inventory)
- [`implementation-terminal-evidence` (Flavor A)](#implementation-terminal-evidence-flavor-a)
- [`step-17a-testing-evidence-gate` (Flavor B)](#step-17a-testing-evidence-gate-flavor-b)
- [Degrade output contract](#degrade-output-contract)
- [Security invariants](#security-invariants)
- [Regression contract](#regression-contract)
- [SKILL mirror parity](#skill-mirror-parity)
- [Scope and non-goals](#scope-and-non-goals)
- [See also](#see-also)

---

## Problem this solves

A bookkeeping or evidence-collection step could abort the whole recipe and
**discard fully-implemented, tested, and committed work**. Two recurring
flavors were observed on real runs:

- **Flavor A — helper-path resolution too narrow (`exit 2`).** A gate resolved
  its bundled helper with only one or two candidate paths, then hard-failed
  with `exit 2` when neither existed. In a downstream *product* worktree whose
  `amplifier-bundle/tools/` is gitignored (a fresh `git worktree add` yields an
  empty `tools/`), resolution never reached the real install root
  (`~/.amplihack/amplifier-bundle/tools/…`). The gate aborted **after**
  implementation and verification had already succeeded, killing
  `smart-orchestrator` and throwing away the branch.

- **Flavor B — a gate hard-fails and cascades when the code work is done and the
  step is legitimately N/A.** `step-17a-testing-evidence-gate` exited non-zero
  immediately after the preceding agent reported "19/19 tests pass" and emitted
  `{"verdict":"WORK_VERIFIED"}`, even though the step itself concluded "No work
  needed for this step." The non-zero exit cascaded up
  `step-17a → workflow-pr-review → execute-single-round-1-development →
  smart-execute-routing → whole recipe FAILED`, discarding a committed fix.

Both flavors share one root cause: a **bookkeeping step's inability to run (or
its inapplicability) was treated as a verification failure of the code.** The
fix separates those two concerns everywhere, systematically.

---

## The two invariants

**Invariant 1 — Robust helper resolution.** A bundled helper that exists at the
install root is *always* found, regardless of `cwd`, `REPO_PATH`, or a
gitignored worktree bundle. Every bundled-helper site uses the same
[canonical resolution ladder](#canonical-resolution-ladder).

**Invariant 2 — No-discard.** A bookkeeping/evidence step's failure MUST NOT
discard validated implementation work. The workflow distinguishes:

| Class | Example | Terminal action |
| --- | --- | --- |
| (a) genuine **code**-verification failure | tests failing, implementation missing, `HOLLOW_SUCCESS` verdict | **FATAL** — stays `exit 1`/`exit 2`. |
| (b) bookkeeping/evidence step that **could not run** or found **no applicable work** | helper unresolvable; N/A external-integration gate on a change with no external integration | **DEGRADE** — loud `WARNING:` + `exit 0`, workflow proceeds and the branch/PR is preserved. |

Degradation is **never silent**. A degrade always emits a `WARNING:`-prefixed
line on stderr, and — where the step's stdout is consumed (e.g. a `parse_json`
step) — a machine-consumable annotation on stdout as well. A silent skip is a bug
(it is the inverse of the original #962 defect).

---

## Canonical resolution ladder

Every site that invokes a bundled `amplifier-bundle/tools/*.sh` helper resolves
it through the same ladder, in this exact order (mirroring the landed
`workflow_runtime_artifacts.sh` sibling). The first existing path wins:

| Tier | Candidate | Rationale |
| --- | --- | --- |
| 1 | `${AMPLIHACK_HOME:-${REPO_PATH:-$(pwd)}}/amplifier-bundle/tools/<helper>.sh` | Explicit install root set by the launcher, falling back to the repo checkout, then cwd. |
| 2 | `${REPO_PATH:-$(pwd)}/amplifier-bundle/tools/<helper>.sh` | The task's repo checkout, when it carries the bundle. |
| 3 | `$(pwd)/amplifier-bundle/tools/<helper>.sh` | Current working directory. |
| 4 | `${HOME:-/root}/.copilot/amplifier-bundle/tools/<helper>.sh` | Copilot CLI install location. |
| 5 | `${HOME:-/root}/.amplihack/amplifier-bundle/tools/<helper>.sh` | Default amplihack install location — the tier that rescues gitignored-bundle worktrees. |

The git-identity chain (#955) uses the same shape but additionally interposes a
`$(git rev-parse --show-toplevel)` tier; both variants share the "install-root
tier is always present, one terminal action last" contract.

Exactly **one** terminal action follows the ladder (see
[terminal-action policy](#terminal-action-policy-fatal-vs-degrade)). The ladder
order and the "one terminal action last" rule mirror the landed #955
git-identity chain, so all sibling helpers share one shape.

### Reference shape (bookkeeping/evidence helper — degrade on miss)

```bash
set -euo pipefail
H="${AMPLIHACK_HOME:-${REPO_PATH:-$(pwd)}}/amplifier-bundle/tools/workflow_implementation_evidence.sh"
[ -f "$H" ] || H="${REPO_PATH:-$(pwd)}/amplifier-bundle/tools/workflow_implementation_evidence.sh"
[ -f "$H" ] || H="$(pwd)/amplifier-bundle/tools/workflow_implementation_evidence.sh"
[ -f "$H" ] || H="${HOME:-/root}/.copilot/amplifier-bundle/tools/workflow_implementation_evidence.sh"
[ -f "$H" ] || H="${HOME:-/root}/.amplihack/amplifier-bundle/tools/workflow_implementation_evidence.sh"
if [ -f "$H" ]; then
  bash "$H"                                    # helper emits its evidence JSON
else
  echo "WARNING: implementation evidence helper not found …; degrading VISIBLY and continuing to preserve validated work (no-discard, #962)" >&2
  printf '%s\n' '{"implementation_completed":"true","terminal_state":"EVIDENCE_TOOL_UNAVAILABLE", …}'
fi
```

> **Note.** `workflow_runtime_artifacts.sh` appears at *both* shapes: the
> mid-flight preflight-enrichment sites (tdd checkpoint, `workflow-finalize`)
> use the **degrade** shape, while the pre-publish provenance gates
> (`workflow-publish`, `workflow-refactor-review`, `workflow-pr-review`) use the
> **fatal** shape per the landed #829 contract. The ladder is identical; only the
> terminal action differs by the gate's role.

### Reference shape (code-verification / provenance gate — fatal on miss)

```bash
set -euo pipefail
H="${AMPLIHACK_HOME:-${REPO_PATH:-$(pwd)}}/amplifier-bundle/tools/<verify_helper>.sh"
# … tiers 2–5 (REPO_PATH, pwd, ~/.copilot, ~/.amplihack) …
[ -f "$H" ] || { echo "ERROR: <verify_helper> not found: $H" >&2; exit 2; }
bash "$H"
```

> **Quoting is mandatory.** Every candidate expands
> `"${AMPLIHACK_HOME}"`, `"${REPO_PATH}"`, `"$(pwd)"`, `"$HOME"` inside double
> quotes. Helpers are resolved by explicit absolute path only — never a bare
> filename or `PATH` lookup — to prevent helper hijack.

---

## Terminal-action policy: FATAL vs DEGRADE

A bundled-helper site degrades **only** when both are true:

1. its job is pure **bookkeeping / evidence recording** (not verifying code
   correctness or commit provenance), **and**
2. aborting it would **discard already-validated but not-yet-published work** —
   i.e. it runs mid-flight, before the branch/PR is pushed.

- **DEGRADE** applies to those mid-flight bookkeeping/evidence gates:
  `implementation-terminal-evidence`, `step-17a-testing-evidence-gate` (its N/A
  path), the runtime-artifact *preflight enrichment* at the tdd checkpoint and in
  `workflow-finalize`, and the best-effort self-healing worktree sweep. An
  unresolvable helper or a not-applicable outcome emits a `WARNING:` to stderr
  and continues (`exit 0`) so the committed work is preserved.

- **FATAL** applies to (a) code-verification / provenance gates whose non-zero
  exit means the *code is wrong, unverified, or un-attributed*, and (b) the
  actual **publish / finalize / provenance operations** whose failure is a
  genuine "cannot complete" — these run *after* the branch is already pushed, so
  a loud abort surfaces the break without discarding the branch. These keep their
  terminal `exit 1`/`exit 2`.

Every site carries the canonical 6-tier ladder regardless of terminal action, so
the helper is always found at the install root; the terminal only fires on a
genuinely broken install. A single site never mixes both terminal actions.

---

## The FATAL allowlist

These gates are **inviolable** — they stay fatal. Softening any of these is a
regression that the [fatal-allowlist test](#regression-contract) rejects.

| Gate | Recipe | Why fatal |
| --- | --- | --- |
| `step-08c-enforce-verdict` | `workflow-tdd.yaml` | Rejects `HOLLOW_SUCCESS` — a real verification failure of the code. |
| `step-19c-zero-bs-verification` | `workflow-pr-review.yaml` | Enforces the Zero-BS / no-stub invariant on the committed code. |
| All git-identity chains (`git-identity.sh`, 7 single-line sites) | 7 recipes (#955) | Commit provenance/identity is an authz control. |
| Pre-publish runtime-artifact provenance gates (`RUNTIME_ARTIFACT_HELPER`) | `workflow-publish.yaml` (×2), `workflow-refactor-review.yaml` (×1), `workflow-pr-review.yaml` (×1) | Landed #829 contract: un-preflighted artifacts must never be pushed. |
| `READY_HELPER`, `FINAL_STATUS_HELPER`, `FINALIZER_HELPER` (×3) | `workflow-finalize.yaml` | Publish/finalize/verdict operations; run after push, so a loud abort does not discard work. |
| `PUBLISH_HELPER` | `workflow-publish.yaml` | The PR-publish operation itself; failure is a genuine "cannot publish". |

`step-17a-testing-evidence-gate` is **not** on the allowlist: a genuine
code-verification failure it detects is still fatal, but "no work needed" and
"evidence tool unresolvable" are degrades (see
[Flavor B](#step-17a-testing-evidence-gate-flavor-b)).

---

## Gate inventory

Every bundled-helper site and gate touched by this feature, with its
before/after resolution and failure behavior. All bookkeeping sites move to the
canonical five-tier ladder; the terminal action follows the
[policy](#terminal-action-policy-fatal-vs-degrade).

| Site (step / helper var) | Recipe | Helper | Before | After |
| --- | --- | --- | --- | --- |
| `implementation-terminal-evidence` | `workflow-tdd.yaml` | `workflow_implementation_evidence.sh` | 2-tier + `exit 2` | 6-tier ladder + **DEGRADE** |
| `RUNTIME_ARTIFACT_HELPER` (checkpoint) | `workflow-tdd.yaml` | `workflow_runtime_artifacts.sh` | canonical ladder + DEGRADE (#829) | unchanged — canonical ladder + DEGRADE |
| `step-17a-testing-evidence-gate` | `workflow-pr-review.yaml` | (evidence check) | single `exit 1` on any gap | 3-outcome: pass / **DEGRADE** (N/A or empty) / **FATAL** (genuine code-fail) |
| `RUNTIME_ARTIFACT_HELPER` (step-18c) | `workflow-pr-review.yaml` | `workflow_runtime_artifacts.sh` | 5-tier git-toplevel form (`AMPLIHACK_HOME`→git-toplevel→cwd→`.copilot`→`.amplihack`) + `exit 2` | same git-toplevel variant, **FATAL** (#829 pre-publish provenance). Deliberately omits the `REPO_PATH` tier to honor the #684 worktree invariant (`step-18c` must require the worktree). |
| `RUNTIME_ARTIFACT_HELPER` (×3) | `workflow-finalize.yaml` | `workflow_runtime_artifacts.sh` | canonical ladder + DEGRADE (#829) | unchanged — canonical ladder + DEGRADE |
| `READY_HELPER`, `FINAL_STATUS_HELPER`, `FINALIZER_HELPER` (×3) | `workflow-finalize.yaml` | `workflow_pr_ready.sh`, `workflow_final_status.sh`, `workflow_agentic_finalization.sh` | 2-tier `if`-form + `exit 1`/`2` | 6-tier ladder + **FATAL** (publish/finalize/verdict op) |
| `RUNTIME_ARTIFACT_HELPER` (×2) | `workflow-publish.yaml` | `workflow_runtime_artifacts.sh` | canonical ladder + `exit 2` | unchanged — canonical ladder + **FATAL** (#829) |
| `PUBLISH_HELPER` | `workflow-publish.yaml` | `workflow_publish_pr.sh` | 2-tier `if`-form + `exit 1` | 6-tier ladder + **FATAL** (publish op) |
| `SWEEP_HELPER` | `workflow-worktree.yaml` | `workflow_worktree_sweep.sh` | 2-tier, `if`-guarded no-op | 6-tier ladder, `if`-guarded best-effort (**DEGRADE** / no-op) |
| `RUNTIME_ARTIFACT_HELPER` | `workflow-refactor-review.yaml` | `workflow_runtime_artifacts.sh` | canonical ladder + `exit 2` | unchanged — canonical ladder + **FATAL** (#829) |
| `step-08c-enforce-verdict` | `workflow-tdd.yaml` | — | `exit 1` | **unchanged (FATAL)** |
| `step-19c-zero-bs-verification` | `workflow-pr-review.yaml` | — | `exit 1` | **unchanged (FATAL)** |
| `PR_SCOPE_HELPER` | `workflow-terminal-state.yaml` | `workflow_pr_scope.sh` | 2-tier `if`-form + `fail_terminal_state` | 6-tier ladder + **FATAL** (scope/provenance classifier) |
| `helper` (strict terminal gate) | `workflow-terminal-state.yaml` | `workflow_final_status.sh` | 3-tier `if/elif` + `exit 2` | 6-tier ladder + **FATAL** (post-push terminal gate) |
| git-identity chains (7 sites) | 7 recipes | `git-identity.sh` | 5-tier + `exit 2` (#955) | **unchanged (FATAL)** |

The pre-publish runtime-artifact provenance gates in `workflow-publish.yaml`,
`workflow-refactor-review.yaml`, and `workflow-pr-review.yaml` stay **fatal** per
the landed #829 contract (un-preflighted artifacts must never be pushed); this
sweep only widens their resolution ladder so the fatal never fires for a
correctly installed amplihack. The two `workflow-terminal-state.yaml` classifier
sites (`workflow_pr_scope.sh`, `workflow_final_status.sh`) previously stopped at a
2–3-tier resolver that could not reach the install root — this sweep extends both
to the full ladder (adding the `~/.copilot` and `~/.amplihack` tiers) so a
gitignored-bundle worktree resolves the helper and the terminal-state machine
does not mis-classify a bookkeeping-tool miss as a `FAILED` state. Their terminal
action stays **fatal/fail-visible** because they run at or after publish, where a
loud abort no longer discards un-pushed work.

---

## `implementation-terminal-evidence` (Flavor A)

Step in `workflow-tdd.yaml`. It records terminal evidence that implementation
completed (or was a legitimate no-op). It is **bookkeeping**, so it degrades.

| Property | Value |
| --- | --- |
| `id` | `implementation-terminal-evidence` |
| `type` | `bash` |
| `output` | `implementation_terminal_evidence` |
| Helper | `workflow_implementation_evidence.sh` |
| Resolution | canonical 6-tier ladder |
| Shell mode | `set -euo pipefail`, with the degrade behind an `if [ -f "$HELPER" ]; then … else … fi` guard so the miss branch warns rather than aborts |
| On helper resolved | runs the helper; emits the evidence JSON. |
| On helper unresolvable | `WARNING: workflow implementation evidence helper not found … degrading VISIBLY and continuing to preserve validated implementation work (no-discard invariant, issue #962)` (stderr) **plus** a schema-compatible evidence record on stdout (`terminal_state:"EVIDENCE_TOOL_UNAVAILABLE"`, `implementation_completed:"true"`) so the `parse_json` step still parses; `exit 0`. |

> This step is `parse_json: true`, so its degrade emits a **valid evidence JSON
> object** on stdout (not a bare `DEGRADED:` line). The uniform, machine-greppable
> degrade signal is the `WARNING:` on **stderr**.

The helper's own logic is unchanged (fixed-string parsing via `grep -F` /
`jq -r`, the `ALLOW_NO_OP` and orchestration-sentinel no-op paths). Only its
resolution and the terminal action changed.

---

## `step-17a-testing-evidence-gate` (Flavor B)

Step in `workflow-pr-review.yaml`. It checks that Step 13 (outside-in local
testing) produced evidence before the PR-review phase. It now resolves to one
of three explicit outcomes:

| # | Condition | stdout | stderr | Exit |
| --- | --- | --- | --- | --- |
| 1 | Applicable **and** evidence present (`local_testing_gate` populated, no failure verdict) | `=== Testing-Evidence Gate PASSED ===` | — | `0` |
| 2 | **N/A** / empty gate (`local_testing_gate` unset or `''`) | `=== Testing-Evidence Gate DEGRADED (non-fatal; validated work preserved) ===` | multi-line `WARNING:` naming the degrade + remediation hint | `0` |
| 3 | Genuine **code**-verification failure explicitly recorded in the evidence (e.g. `VERDICT: FAILED`, `N tests failed`) | — | `TESTING-EVIDENCE GATE FAILURE: … explicit test/verification FAILURE` | `1` |

Order matters: outcome 3 (explicit failure verdict) is checked **before** the
empty-gate degrade, so a real failing-tests verdict can never be masked by the
N/A path. Outcome 2 is the fix for the cited cascade: a change that legitimately
needs no external-integration testing no longer aborts `workflow-pr-review` and
discards the committed fix. Outcome 3 preserves the real safety property — an
evidence record that reports failing tests still fails loudly.

---

## Degrade output contract

The uniform, machine-greppable degrade signal is a loud `WARNING:` line on
**stderr** — this is what every regression test asserts. The accompanying
**stdout** is gate-appropriate, because some gates are `parse_json`:

| Gate | stderr signal | stdout on degrade |
| --- | --- | --- |
| `implementation-terminal-evidence` (`parse_json`) | `WARNING: … no-discard invariant, issue #962` | schema-compatible evidence JSON (`terminal_state:"EVIDENCE_TOOL_UNAVAILABLE"`) |
| `step-17a-testing-evidence-gate` | multi-line `WARNING:` | `=== Testing-Evidence Gate DEGRADED (non-fatal; validated work preserved) ===` |
| runtime-artifact preflight enrichment (tdd checkpoint, finalize) | `WARNING: … continuing in degraded mode` | — (stderr-only) |
| worktree sweep | best-effort; `if`-guarded no-op | — |

Rules:

- The `WARNING:` text is **fixed**; untrusted values (helper paths, gate
  contents) are printed via shell expansion of already-quoted variables, never
  interpolated into a format string, and never `eval`-ed.
- A degraded-success run is identified by a `WARNING:` in the run log **alongside**
  the normal evidence of completed implementation, verification, and PR work.
  Reviewers can grep the run log for `WARNING:` to audit every bookkeeping step
  that could not run.
- No secret leakage: degrade output names the helper/gate and outcome only —
  never `GITHUB_TOKEN`, `GH_TOKEN`, env/`set` dumps, or other users' home paths.

---

## Security invariants

| ID | Invariant |
| --- | --- |
| SEC-AUTHZ-1 | The degrade contract never weakens provenance/integrity gates. git-identity and `step-19c` stay FATAL — identity is an authz control, not bookkeeping. |
| SEC-INPUT-1 | Every resolution variable is double-quoted (`"${AMPLIHACK_HOME}"`, `"${REPO_PATH}"`, `"$(pwd)"`, `"$HOME"`) to prevent word-splitting/injection. |
| SEC-INPUT-2 | Helpers are resolved by explicit absolute path (`"$tier/amplifier-bundle/tools/<helper>.sh"`) only — never a bare filename or `PATH` lookup (prevents helper hijack). |
| SEC-INPUT-3 | Helper filenames are hard-coded literals; no dynamic construction. |
| SEC-INPUT-4 | No `eval`, no `source` of untrusted input, no `${!var}` / `${var@P}` / chained-substitution constructs. |
| SEC-DATA-1 | Degrade output is helper name + outcome only; no env or secret dumps. |
| SEC-DATA-2 | `workflow_runtime_artifacts.sh` cleanup scope is unchanged: `path_is_tracked` fail-closed guard and linked-worktree gating are preserved; no `rm -rf` widening. |
| SEC-R5 | The degrade branch is guarded by `if [ -f "$H" ]; then … else WARN … fi` (the miss branch has no unconditional `exit`), so an unresolvable bookkeeping helper warns and the step continues rather than aborting. |

---

## Regression contract

Tests live under `amplifier-bundle/recipes/tests/`, mirror the
`test-issue-955-git-identity-fallback.sh` harness style (isolated `mktemp -d`
fixtures, a grep/awk-extracted step body executed via `printf … | bash` in a
subshell), and are registered in `.github/workflows/ci.yml` next to the #955
test so CI actually runs them.

Each touched gate is covered by these contracts:

| Contract | Asserts |
| --- | --- |
| **POS** | The gate resolves and runs its helper when the bundle exists **only** at the install root (`${HOME}/.amplihack/…` and `${HOME}/.copilot/…`), reproducing the gitignored-worktree scenario. |
| **N/A** | An inapplicable/empty gate emits a loud `WARNING:` + `exit 0` and does **not** abort the recipe. |
| **FAIL-VISIBLE** | A genuinely missing helper on a FATAL site, **or** a real code-verification failure (`VERDICT: FAILED`, `HOLLOW_SUCCESS`), still fails loudly (`exit 1`/`exit 2` + `ERROR`/`WARNING`). |
| **NO-DISCARD** | A bookkeeping-step failure does not throw away a committed, verified implementation — the workflow proceeds to finalization/summary. |

Test files:

| File | Focus |
| --- | --- |
| `test-issue-962-implementation-evidence-fallback.sh` | Flavor A 6-tier ladder + degrade (no bare `exit 2`) for `implementation-terminal-evidence`. |
| `test-issue-962-step17a-testing-evidence-gate.sh` | Flavor B three-outcome behavior (pass / degrade / fatal-on-failure-verdict). |
| `test-issue-962-runtime-artifact-ladders.sh` | Canonical `REPO_PATH` ladder tiers across finalize/publish/worktree/refactor-review/tdd; the git-toplevel variant (no `REPO_PATH`, #684) for pr-review `step-18c`; and dynamic install-root (`~/.amplihack`) resolution. |
| `test-issue-962-fatal-allowlist-preserved.sh` | `step-08c`, `step-19c`, 7 git-identity sites still fatal; converted gates degrade; CI wired. |
| `test-issue-962-skill-mirror-parity.sh` | Both `SKILL.md` mirrors stay byte-identical and document the policy. |

A test that is not wired into `ci.yml` is treated as zero-value; registration is
part of the contract.

---

## SKILL mirror parity

The `default-workflow` skill is mirrored in two locations that MUST stay
identical (prior churn in #849/#852 was caused by drift between them):

- `amplifier-bundle/skills/default-workflow/SKILL.md`
- `docs/claude/skills/default-workflow/SKILL.md`

> **Implementer note.** These are the two mirrors that actually exist in the
> tree. An early design draft referenced
> `amplifier-bundle/.claude/skills/default-workflow/SKILL.md`; that path does
> **not** exist — do not create it. Reconcile only the two paths above.

Both mirrors document the FATAL-vs-DEGRADE gate policy described here, and
`test-issue-962-skill-mirror-parity.sh` asserts they are byte-identical.

---

## Scope and non-goals

| In scope | Out of scope |
| --- | --- |
| Canonical 5-tier ladder on every bundled-helper site in the touched recipes. | Runner/Rust changes; `Cargo.toml` version bumps. |
| No-discard invariant: bookkeeping degrade vs code-verification fatal. | Softening the FATAL allowlist (git-identity, `step-08c`, `step-19c`). |
| Flavor A (`implementation-terminal-evidence`) and Flavor B (`step-17a`) fixes. | Any Python or kuzu usage; anything named "Bridge". |
| Regression tests per gate + CI registration; SKILL mirror parity. | Wall-clock timeouts on agentic steps (idle/liveness detection only). |
| YAML + shell-helper changes in `rysweet/amplihack-rs`. | Downstream product repos (e.g. Simard) — untouched. |

---

## See also

- [Workflow Runtime Artifacts Reference](workflow-runtime-artifacts.md) — the
  runtime-root and cleanup helper whose ladder this feature normalizes.
- [Non-Fatal Documentation Review Checkpoint Reference](doc-review-non-fatal-checkpoint.md)
  — the sibling non-fatal-gate pattern (#834).
- [Workflow Commit Identity](workflow-commit-identity.md) — the git-identity
  ladder (#955) whose shape this feature mirrors.
- [Workflow Terminal State](workflow-terminal-state.md) — terminal-success
  gating that stays fail-closed.
