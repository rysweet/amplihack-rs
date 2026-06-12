# step-03-create-issue: Host-Aware Tracking Idempotency

`step-03-create-issue` is the tracking-record step in `workflow-prep.yaml`,
the preparation phase used by `default-workflow`. It creates or reuses the
tracking record for the current workflow run:

- GitHub remotes use GitHub Issues.
- Azure DevOps remotes use Azure Boards work items.
- Unknown, empty, or local remotes use structured local metadata.

Since `default-workflow` is often re-run against the same task (for example
when resuming after an interruption, retrying a failed step, or following up on
an existing PR/work item), the step detects existing tracking references before
creating anything new.

**Added in:** PR #3952 (merged 2026-04-03)
**Pattern source:** `step-16-create-draft-pr` idempotency guards (#3324)

---

## Quick Start

No configuration is required for GitHub repositories. Step 02d detects the
remote host and step 03 routes by `remote_host_type`.

```bash
# GitHub: reuse issue #4194 if it exists, otherwise search/create as needed
amplihack recipe run default-workflow \
  -c task_description="Fix login timeout bug in #4194" \
  -c repo_path="$(pwd)"
```

Azure DevOps repositories may use either `azdo` or `azure-devops` as the host
type. Both values route to the Azure Boards path.

```bash
# Azure DevOps: reuse existing work item 12345 without creating a GitHub issue
amplihack recipe run default-workflow \
  -c remote_host_type=azdo \
  -c issue_number=12345 \
  -c task_description="Continue ADO PR follow-up work" \
  -c repo_path="$(pwd)"
```

Step 03 emits a parseable tracking reference, and step 03b extracts the same
numeric ID for downstream branch, commit, and PR logic.

```
AB#12345
```

---

## How It Works

The step dispatches by host type before it performs any provider-specific
operation. `remote_host_type` is treated as untrusted recipe context, so the
dispatch uses quoted variables and explicit host matching.

```
input: remote_host_type + issue_number + task_description + repo_path
         │
         ▼
┌─────────────────────────────────────────────────────────────────┐
│  Host Dispatch                                                  │
│  github              → GitHub issue reuse/search/create          │
│  azdo|azure-devops   → Azure Boards reuse/create                 │
│  other|empty|unknown → structured local metadata                 │
└─────────────────────────────────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────────────────────────────────┐
│  Existing Reference Guards                                      │
│  explicit issue_number? host-specific reuse, exit 0              │
│  task_description contains AB#N or #N? host-specific candidate   │
└─────────────────────────────────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────────────────────────────────┐
│  Provider Create or Fallback                                    │
│  GitHub: gh issue create                                        │
│  Azure DevOps: az boards work-item create                       │
│  Other: structured local metadata                               │
└─────────────────────────────────────────────────────────────────┘
```

### Host Dispatch

Step 03 accepts these `remote_host_type` values:

| Value | Meaning | Step 03 behavior |
| ----- | ------- | ---------------- |
| `github` | GitHub repository | Runs GitHub issue reuse/search/create logic with `gh` |
| `azdo` | Azure DevOps repository | Runs Azure Boards work-item reuse/create logic |
| `azure-devops` | Azure DevOps repository alias | Same behavior as `azdo` |
| `other` | Unknown or local repository | Uses local tracking fallback |
| empty/unset | Unknown or local repository | Uses local tracking fallback |

Step 02d normally emits `github`, `azdo`, or `other`. The `azure-devops` alias
exists for callers that pass host context from Azure DevOps PR/work-item
follow-up workflows.

### GitHub Guard 1: Reference Guard

Triggered when the host is `github` and `task_description` contains a GitHub
issue reference in the form `#NNNN` (for example, `Fix the bug in #4194`).

1. Extracts the first `#NNNN` pattern using bash regex `[[ =~ \#([0-9]+) ]]`
2. Validates the extracted value is purely numeric (defense-in-depth)
3. Calls `gh issue view <N> --json url --jq '.url // ""'` with a 60-second timeout
4. If the issue exists: outputs its URL to stdout and exits 0 (reuse)
5. If the issue does not exist or the call fails: falls through to Guard 2

This guard is the cheapest and most specific. It requires zero search and makes
a single API call to a known issue number.

### GitHub Guard 2: Title Search Guard

Runs only for the `github` host path when Guard 1 does not match. Uses
`gh issue list` to search open issues for a title similar to the current one.

1. Truncates the issue title to its first 100 characters (GitHub search limit)
2. Calls `gh issue list --state open --search "<query>"` with a 60-second timeout
3. If a matching open issue is found: outputs its URL to stdout and exits 0 (reuse)
4. If no match: falls through to issue creation

This guard catches the case where the workflow was re-run without explicitly
referencing an issue number — for example, when the task description is
re-submitted verbatim.

### GitHub Fallback: Create New Issue

If neither guard matches, the step creates a new issue using the same logic
as before the idempotency guards were added. This path is unchanged for GitHub
repositories.

### Azure DevOps Existing Work Item Reuse

Runs when `remote_host_type` is `azdo` or `azure-devops`.

The Azure DevOps path never calls `gh issue view`, `gh issue list`, or
`gh issue create`. Existing work-item reuse is checked before any create
operation:

1. If the recipe context already contains numeric `issue_number=N`, step 03
   emits `AB#N` and exits 0 before GitHub logic, Azure CLI lookup, remote URL
   parsing, or work-item creation.
2. Otherwise, if `task_description` contains `AB#N`, step 03 reuses that work
   item reference when the Azure Boards path can validate or resolve it.
3. Otherwise, if `task_description` contains `#N`, step 03 treats it as an
   Azure Boards work-item candidate only because the host dispatch already
   selected Azure DevOps.

Only explicit `issue_number=N` is trusted as an already-known workflow context
value. IDs discovered in `task_description` are provider-scoped candidates:
they must stay in the Azure DevOps branch, but they may still fall through to
work-item creation or local tracking if Azure CLI, organization, project, or
work-item resolution is unavailable.

When Azure CLI and the DevOps extension are available, validated referenced
work items may be resolved to full work-item URLs:

```text
https://dev.azure.com/myorg/myproject/_workitems/edit/12345
```

When an existing `issue_number` is already present, no Azure Boards lookup or
create command is needed; the parseable `AB#N` output is enough for step 03b
and all downstream workflow steps.

### Azure DevOps Create Path

If no existing work item is supplied, step 03 parses the Azure DevOps remote
URL to derive organization and project, then creates a `Task` work item with
`az boards work-item create`. Supported remote URL forms are:

| Form | Example |
| ---- | ------- |
| Modern HTTPS | `https://dev.azure.com/myorg/MyProject/_git/myrepo` |
| Legacy HTTPS | `https://myorg.visualstudio.com/MyProject/_git/myrepo` |
| SSH | `git@ssh.dev.azure.com:v3/myorg/MyProject/myrepo` |

Percent-encoded project names such as `My%20Project` are decoded before
validation. Invalid org/project captures fall back to local metadata with a
warning rather than crossing into GitHub logic.

### Local Metadata Fallback

Unknown hosts, empty hosts, non-git directories, malformed Azure DevOps remote
metadata, or unavailable Azure CLI support produce structured local metadata:

```text
tracking_system=local
tracking_reference=local-issue-482193
tracking_issue=local-issue-482193
issue_creation=local-tracking
issue_number=482193
```

Local metadata preserves the workflow's numeric `issue_number` contract when a
numeric local reference can be derived from explicit `issue_number`, `AB#N`,
`#N`, or `local-issue-N` / `local-ab-N`. If no numeric local reference is
available, step 03b must fail visibly instead of inventing an ID silently.

---

## Output Format

Step 03 writes provider output to stdout. GitHub and Azure Boards success paths
write a single URL or `AB#N`; local fallback writes multiline key/value
metadata. Diagnostic output goes to stderr.

| Host path | Reuse output | Create output |
| --------- | ------------ | ------------- |
| GitHub | `https://github.com/owner/repo/issues/123` | `https://github.com/owner/repo/issues/123` |
| Azure DevOps | `AB#12345` for explicit `issue_number`, or `https://dev.azure.com/org/project/_workitems/edit/12345` for validated task-text reuse | `https://dev.azure.com/org/project/_workitems/edit/12345` |
| Other/local | Structured local metadata | Structured local metadata |

The downstream step `step-03b-extract-issue-number` accepts every output above.
It extracts the numeric ID from:

- GitHub issue URLs containing `/issues/N`
- GitHub PR URLs containing `/pull/N` with closing-issue lookup fallback
- Azure DevOps work-item URLs containing `/_workitems/edit/N`
- Azure Boards references in the form `AB#N`
- Local metadata containing `issue_number=N`
- Local metadata references in the form `tracking_reference=local-issue-N` or `tracking_reference=local-ab-N`
- Legacy local tracking references in the form `local-tracking:N`

This keeps the downstream `issue_number` output provider-agnostic.

---

## Diagnostic Messages

All diagnostic output goes to **stderr** and is not captured by the recipe
runner's output pipeline. You can view it in the recipe's verbose log or by
redirecting stderr. The table below is the expected diagnostic contract for
the Issue #718 implementation; exact wording should not be treated as a stable
public API.

| Message                                                                      | When                             |
| ---------------------------------------------------------------------------- | -------------------------------- |
| `INFO: task_description references issue #N — verifying it exists`           | Guard 1 extracted a reference    |
| `INFO: Reusing existing issue #N — skipping creation`                        | Guard 1 matched and reused       |
| `WARN: Referenced issue #N not found — will search or create`                | Guard 1 fell through             |
| `INFO: Searching open issues for similar title`                              | Guard 2 running                  |
| `INFO: Found existing open issue matching title — skipping creation`         | Guard 2 matched and reused       |
| `INFO: No matching open issue found — proceeding to create`                  | Guard 2 fell through             |
| `WARN: Extracted issue reference is not numeric: <value> — skipping guard 1` | Guard 1 rejected an unsafe value |
| `INFO: Reusing work item AB#N`                                               | Azure DevOps path reused a work item |
| `INFO: Using local tracking for issue management (remote: HOST)`             | Fallback path selected local tracking |
| `WARN: 'az' CLI not found or org/project empty — using local tracking for AzDO remote` | Azure DevOps create path could not run |

---

## Error Handling

| Failure mode | Behavior |
| ------------ | -------- |
| `gh issue view` times out (> 60 s) | GitHub Guard 1 falls through |
| `gh issue view` returns HTTP error | GitHub Guard 1 falls through |
| `gh issue list --search` times out | GitHub Guard 2 falls through |
| `gh issue list --search` returns empty | GitHub Guard 2 falls through to creation |
| `gh` not authenticated on GitHub path | Reuse guards fall through; creation fails clearly if authentication is required |
| `gh issue create` cannot resolve or access the repository | Emits structured local metadata with a warning |
| Other `gh issue create` failure | Fails clearly and prints sanitized CLI output |
| `remote_host_type=azdo` or `azure-devops` with existing `issue_number` | Emits `AB#N` and exits 0 without calling `gh` or creating a work item |
| Azure CLI missing on Azure DevOps create path | Falls back to structured local metadata with a warning |
| Azure DevOps org/project cannot be parsed | Falls back to structured local metadata with a warning |
| Non-numeric issue reference extracted | Explicit `^[0-9]+$` validation rejects it before any provider CLI receives it |

The step uses `set -euo pipefail`. All expected-failure exit paths use
`|| echo ''` or `|| true` so the script does not abort unexpectedly.

---

## Security

### Command Injection Prevention

| Attack vector                                                      | Mitigation                                                                                                                                                                                                                     |
| ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `#NNNN` in `task_description` contains shell metacharacters        | Bash regex `[[ =~ \#([0-9]+) ]]` captures only `[0-9]+`; `BASH_REMATCH[1]` contains only digits                                                                                                                                |
| Captured number contains semicolons, pipes, or other characters    | Explicit `^[0-9]+$` validation rejects anything non-numeric before it reaches `gh issue view "$REF_ISSUE_NUM"`                                                                                                                 |
| Long or special-character title passed to `gh issue list --search` | Double-quoted variable `"$SEARCH_QUERY"` prevents shell word-splitting; `gh` CLI handles API-level escaping                                                                                                                    |
| Template injection via `task_description` or `final_requirements`  | Both are captured via unquoted heredoc (`<<EOFTASKDESC`) into bash variables (`TASK_DESC`, `ISSUE_REQS`). The issue body is assembled with `printf` using double-quoted variable expansions — no `eval`, no unquoted expansion |
| Untrusted `remote_host_type` value | Quoted host dispatch routes only explicit `github`, `azdo`, and `azure-devops` values to provider logic; all other values use local tracking |
| Azure DevOps alias with shell metacharacters | The alias match is exact; values such as `azure-devops; gh issue create ...` do not match and fall back to local tracking |

### Trusted Inputs

The recipe context variables `task_description` and `final_requirements` must
never contain secrets or authentication tokens. They are embedded verbatim in
new GitHub issue bodies or Azure Boards work-item descriptions. Public
repositories and shared Azure DevOps projects may expose that text to broad
audiences.

---

## Configuration

Step 03 reads these recipe context keys:

| Context key | Required | Description |
| ----------- | -------- | ----------- |
| `repo_path` | Yes | Repository or worktree path where `git remote get-url origin` runs |
| `task_description` | Yes | Free-form task text used for title creation and existing reference extraction |
| `final_requirements` | No | Requirements text included in newly created GitHub issues or Azure Boards work items |
| `remote_host_type` | No | Host routing value; accepts `github`, `azdo`, `azure-devops`, `other`, or empty |
| `issue_number` | No | Existing tracking ID. On Azure DevOps, this is reused as `AB#N` without GitHub issue logic |

Step 02d normally sets `remote_host_type`. Callers may override it when
resuming from external workflow context.

```bash
# Explicit Azure DevOps host and existing work item reuse
amplihack recipe run default-workflow \
  -c remote_host_type=azdo \
  -c issue_number=12345 \
  -c task_description="Follow up on existing Azure Boards work item" \
  -c repo_path="$(pwd)"
```

GitHub behavior remains unchanged when `remote_host_type=github`.

---

## Usage Examples

### Example 1: Re-running a workflow for the same task

A previous run created issue #4194. The next run's `task_description` still
references `#4194`.

```
task_description = "Fix login timeout bug described in #4194"
```

**Step-03 output (stderr):**

```
INFO: task_description references issue #4194 — verifying it exists
INFO: Reusing existing issue #4194 — skipping creation
```

**Step-03 output (stdout):**

```
https://github.com/myorg/myrepo/issues/4194
```

No duplicate issue created. Step-03b extracts `4194` as normal.

---

### Example 2: Azure DevOps PR follow-up with an existing work item

An Azure DevOps follow-up workflow already knows the Boards work item ID from
the PR or workstream context.

```bash
amplihack recipe run default-workflow \
  -c remote_host_type=azdo \
  -c issue_number=12345 \
  -c task_description="Address review feedback for the Azure DevOps PR" \
  -c repo_path=/worktrees/ado-pr-follow-up
```

**Step-03 output (stdout):**

```text
AB#12345
```

The GitHub issue path is not entered. No `gh issue` command runs, and no
duplicate Azure Boards work item is created.

---

### Example 3: Azure DevOps task description reference

```bash
amplihack recipe run default-workflow \
  -c remote_host_type=azdo \
  -c task_description="Fix pipeline timeout described in AB#12345" \
  -c repo_path="$(pwd)"
```

Step 03 routes to the Azure DevOps path because the host is `azdo`. The
`AB#12345` reference is treated as an Azure Boards candidate, never as a
GitHub issue. If Azure Boards lookup resolves the work item, the workflow uses
that ID; otherwise the Azure DevOps create/fallback path continues without
crossing into GitHub logic.

---

### Example 4: Re-running GitHub without an explicit issue reference

Previous run created issue #4200 with title `"Add user profile page"`. New run
has the same `task_description` but no `#NNNN` reference.

**Guard 1:** No `#NNNN` found — skips to Guard 2.

**Guard 2 search query:** `Add user profile page` (under 100 chars, no truncation)

**Step-03 output (stderr):**

```
INFO: Searching open issues for similar title
INFO: Found existing open issue matching title — skipping creation
```

**Step-03 output (stdout):**

```
https://github.com/myorg/myrepo/issues/4200
```

---

### Example 5: First GitHub run — no existing issue

No prior issues match. Both guards fall through; a new issue is created.

**Step-03 output (stderr):**

```
INFO: Searching open issues for similar title
INFO: No matching open issue found — proceeding to create
```

**Step-03 output (stdout):**

```
https://github.com/myorg/myrepo/issues/4201
```

---

### Example 6: Unknown host fallback

```bash
amplihack recipe run default-workflow \
  -c remote_host_type=gitlab \
  -c task_description="Add config parser #482193" \
  -c repo_path="$(pwd)"
```

**Step-03 output (stdout):**

```text
tracking_system=local
tracking_reference=local-issue-482193
tracking_issue=local-issue-482193
issue_creation=local-tracking
issue_number=482193
```

Unknown host values never enter GitHub or Azure DevOps provider logic.

---

### Example 7: Very long task description

`task_description` is 500 characters long. The issue title is truncated to 200
characters (recipe-level truncation). Guard 2's search query uses only the
first 100 characters of that title.

```bash
# Title: first 200 chars of task_description
# Search: first 100 chars of title
SEARCH_QUERY="${ISSUE_TITLE:0:100}"
```

This ensures the `gh` search API is not passed excessively long queries.

---

## Testing

The outside-in test suite covers all three code paths and all cross-cutting
concerns:

```bash
# Run the full test suite
gadugi-test run tests/gadugi/step-03-issue-creation-idempotency.yaml --verbose

# Validate the scenario YAML structure
gadugi-test validate tests/gadugi/step-03-issue-creation-idempotency.yaml
```

**Coverage includes:**

| Area                                               | Scenarios |
| -------------------------------------------------- | --------- |
| GitHub Guard 1: `#NNNN` extraction | Existing GitHub issue reuse |
| GitHub Guard 1: numeric validation / injection prevention | Unsafe reference rejection |
| GitHub Guard 2: title truncation | Long title search safety |
| Azure DevOps alias dispatch | `remote_host_type=azdo` and `remote_host_type=azure-devops` route identically |
| Azure DevOps existing context reuse | `issue_number=N` emits `AB#N` without calling `gh` |
| Azure DevOps task text candidates | `AB#N` and host-scoped `#N` references stay in Azure Boards logic and never trigger GitHub issue commands |
| Generic fallback | Unknown, empty, and non-git hosts emit structured local metadata |
| Output compatibility with step-03b | GitHub URL, Azure Boards URL, `AB#N`, structured local metadata, and legacy `local-tracking:N` parse to numeric IDs |
| Host isolation | Azure DevOps and generic paths never execute `gh issue` commands |
| `set -euo pipefail` and quoted host dispatch | Shell syntax remains safe for empty or malformed context |

---

## Known Limitations

**GitHub Guard 2 false positives.** `gh issue list --search` uses GitHub's
full-text search, which can match issues whose titles differ from the current
one. When this happens, step-03 reuses the matched issue instead of creating a
new one. This is intentional: a false-positive reuse is preferable to creating
a duplicate. The matched issue URL is passed downstream as normal, and the
workflow tracks progress there.

**GitHub TOCTOU race.** Between Guard 2's search and `gh issue create`, a
concurrent workflow run could create a matching issue. In that case, two issues
would exist — the same worst-case as before the guards were added. GitHub issue
creation is inherently non-atomic, so this is not mitigated.

**Reference guards use the first matching ID.** If `task_description` contains
multiple `#NNNN` or `AB#NNNN` references, the first host-appropriate reference
is used.

---

## Multi-Provider Note

The GitHub idempotency guards use `gh issue view` and `gh issue list` only
inside the `github` host branch. Azure DevOps host values (`azdo` and
`azure-devops`) use Azure Boards references and never fall through into GitHub
issue logic. See [Multi-Provider Workflow Reference](multi-provider-workflow.md)
for the provider-specific workflow contract.

---

## Related

- `step-16-create-draft-pr` idempotency guards — pattern source (#3324)
- `step-03b-extract-issue-number` — downstream step that parses step-03 output
- `tests/gadugi/step-03-issue-creation-idempotency.yaml` — test suite
- `docs/investigations/step-03-idempotency-guards-analysis.md` — security analysis and implementation notes
- [Multi-Provider Workflow Reference](multi-provider-workflow.md) — provider detection and routing
